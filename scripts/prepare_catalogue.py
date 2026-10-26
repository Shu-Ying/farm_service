"""Build immutable crop releases from this repository's independent source."""

import argparse
import hashlib
from io import BytesIO
import json
import os
from pathlib import Path
import re
import shutil
import tempfile
import zipfile

from PIL import Image

from catalogue_validation import SOIL_MEMBERS, image_size, safe_member, validate_metadata
from publication_lock import publication_lock

LIMITS = {"plants.json": 2 * 1024**2, "metadata.json": 4 * 1024**2,
          "assets.zip": 64 * 1024**2, "land-soils.zip": 64 * 1024**2}
MAX_EXPANDED = 256 * 1024**2
MAX_VALUE = 2**63 - 1
DISPLAY = ("general", "offsetH", "offsetW", "offsetX", "offsetY", "phase")


def json_bytes(value):
    return (json.dumps(value, ensure_ascii=False, sort_keys=True,
                       indent=2, allow_nan=False) + "\n").encode("utf-8")


def digest(content):
    return hashlib.sha256(content).hexdigest()


def read_bounded(path, maximum):
    with path.open("rb") as stream:
        content = stream.read(maximum + 1)
    if not content or len(content) > maximum:
        raise ValueError("empty or oversized file: " + path.name)
    return content


def validate_zip(content):
    total = 0
    names = set()
    with zipfile.ZipFile(BytesIO(content)) as archive:
        for info in archive.infolist():
            safe_member(info.filename)
            if (info.filename in names or info.is_dir() or info.flag_bits & 1
                    or info.file_size > 32 * 1024**2
                    or (info.external_attr >> 16) & 0o170000 == 0o120000):
                raise ValueError("invalid ZIP member")
            names.add(info.filename)
            total += info.file_size
            if total > MAX_EXPANDED:
                raise ValueError("ZIP expanded size exceeds limit")
            picture = archive.read(info)
            with Image.open(BytesIO(picture)) as image:
                image_size(list(image.size))
                image.verify()
    if names != SOIL_MEMBERS:
        raise ValueError("soil ZIP requires exactly the three soil sprites")
    return total


def validate_plants(document):
    if (not isinstance(document, dict) or type(document.get("schema")) is not int
            or document["schema"] != 1 or set(document) != {"schema", "plants"}
            or not isinstance(document["plants"], list) or not document["plants"]):
        raise ValueError("invalid plants document")
    identities, names = {}, set()
    for row in document["plants"]:
        if not isinstance(row, dict):
            raise ValueError("invalid plant")
        for key in ("id", "level", "buy", "isVip", "vipBuy", "experience",
                    "harvest", "price", "time", "crop"):
            if type(row.get(key)) is not int or not 0 <= row[key] <= MAX_VALUE:
                raise ValueError("invalid plant integer: " + key)
        if any(row[key] < 1 for key in ("id", "harvest", "time", "crop")):
            raise ValueError("invalid positive plant integer")
        if row["time"] * 3600 > MAX_VALUE:
            raise ValueError("growth seconds exceed client domain limit")
        for key in ("isVip", "isBuy", "enabled"):
            if type(row.get(key, 1)) is not int or row.get(key, 1) not in (0, 1):
                raise ValueError("invalid plant flag")
        if (type(row.get("crop_type", 1)) is not int
                or not 1 <= row.get("crop_type", 1) <= MAX_VALUE):
            raise ValueError("invalid crop type")
        name = row.get("name")
        if (not isinstance(name, str) or not name.strip() or len(name) > 128
                or any(ord(c) < 32 for c in name) or name in names
                or name in (".", "..") or "/" in name or "\\" in name
                or row["id"] in identities):
            raise ValueError("invalid or duplicate plant identity")
        phase = row.get("phase")
        if not isinstance(phase, str) or not re.fullmatch(r"[0-9]+(,[0-9]+)+", phase):
            raise ValueError("invalid phase")
        phases = list(dict.fromkeys(int(p) for p in phase.split(",")))
        if len(phases) < 2 or min(phases) < 1 or phases != sorted(phases):
            raise ValueError("invalid phase order")
        if max(phases) > MAX_VALUE or sum(phases[1:]) > MAX_VALUE:
            raise ValueError("phase seconds exceed client domain limit")
        for key in DISPLAY[:-1]:
            if type(row.get(key)) is not int:
                raise ValueError("invalid display integer")
        identities[row["id"]] = name
        names.add(name)
    return identities


def validate_manifest(document):
    if (set(document) != {"schema", "version", "release", "files"}
            or type(document["schema"]) is not int or document["schema"] != 1
            or not re.fullmatch(r"[A-Za-z0-9._-]{1,64}", document["version"])
            or set(document["files"]) != set(LIMITS)):
        raise ValueError("invalid manifest")
    for name, descriptor in document["files"].items():
        if (set(descriptor) != {"size_bytes", "sha256"}
                or type(descriptor["size_bytes"]) is not int
                or not 0 < descriptor["size_bytes"] <= LIMITS[name]
                or not re.fullmatch(r"[a-f0-9]{64}", descriptor["sha256"])):
            raise ValueError("invalid file descriptor")
    canonical = json.dumps(document["files"], sort_keys=True,
                           separators=(",", ":")).encode("utf-8")
    if document["release"] != digest(canonical):
        raise ValueError("invalid release digest")


def verified_current(output):
    latest = output / "manifest.json"
    if not latest.exists():
        return None
    content = read_bounded(latest, 16 * 1024)
    manifest = json.loads(content)
    validate_manifest(manifest)
    directory = output / "releases" / manifest["release"]
    if read_bounded(directory / "manifest.json", 16 * 1024) != content:
        raise ValueError("latest manifest differs from immutable release")
    for name, descriptor in manifest["files"].items():
        value = read_bounded(directory / name, LIMITS[name])
        if len(value) != descriptor["size_bytes"] or digest(value) != descriptor["sha256"]:
            raise ValueError("current release is corrupt")
    return directory


def build(source, version):
    if not re.fullmatch(r"[A-Za-z0-9._-]{1,64}", version):
        raise ValueError("invalid version")
    plants = json.loads(read_bounded(source / "plants.json", LIMITS["plants.json"]))
    validate_plants(plants)
    metadata = json.loads(read_bounded(source / "metadata.json", LIMITS["metadata.json"]))
    if type(metadata.get("schema")) is not int or metadata["schema"] != 1:
        raise ValueError("invalid metadata schema")
    metadata["plants"] = {row["name"]: {key: row[key] for key in DISPLAY}
                          for row in plants["plants"]}
    pictures = {}
    metadata["images"] = {}
    total = 0
    for path in sorted((source / "images").rglob("*")):
        if path.is_symlink():
            raise ValueError("symlink in image source")
        if not path.is_file():
            continue
        name = safe_member(path.relative_to(source / "images").as_posix())
        content = read_bounded(path, min(MAX_EXPANDED, 32 * 1024**2))
        total += len(content)
        if total > MAX_EXPANDED:
            raise ValueError("images exceed expanded size limit")
        with Image.open(BytesIO(content)) as image:
            size = list(image.size)
            image_size(size)
            image.verify()
        pictures[name] = content
        metadata["images"][name] = {"size": size, "sha256": digest(content)}
    if not pictures:
        raise ValueError("empty images directory")
    validate_metadata(metadata, plants["plants"])
    buffer = BytesIO()
    with zipfile.ZipFile(buffer, "w", compression=zipfile.ZIP_STORED) as archive:
        for name, content in sorted(pictures.items()):
            info = zipfile.ZipInfo(name, (2020, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_STORED
            info.create_system = 3
            info.external_attr = 0o100644 << 16
            archive.writestr(info, content)
    soils = read_bounded(source / "land-soils.zip", LIMITS["land-soils.zip"])
    if total + validate_zip(soils) > MAX_EXPANDED:
        raise ValueError("combined ZIP expanded size exceeds limit")
    files = {"plants.json": json_bytes(plants), "metadata.json": json_bytes(metadata),
             "assets.zip": buffer.getvalue(), "land-soils.zip": soils}
    descriptors = {}
    for name, content in files.items():
        if len(content) > LIMITS[name]:
            raise ValueError("release file exceeds size limit")
        descriptors[name] = {"size_bytes": len(content), "sha256": digest(content)}
    canonical = json.dumps(descriptors, sort_keys=True, separators=(",", ":")).encode()
    manifest = {"schema": 1, "version": version, "release": digest(canonical),
                "files": descriptors}
    files["manifest.json"] = json_bytes(manifest)
    return manifest, files


def prepare(source, output, version, check=False):
    if check:
        # Read-only checks never create a lock file or an output directory.
        return _prepare(source, output, version, True)
    with publication_lock(output):
        return _prepare(source, output, version, False)


def _prepare(source, output, version, check):
    current = verified_current(output)
    manifest, files = build(source, version)
    for history in (output / "releases").glob("*/manifest.json"):
        if history.parent.name.startswith(".prepare-"):
            continue
        if not re.fullmatch(r"[a-f0-9]{64}", history.parent.name):
            raise ValueError("invalid published release directory")
        old = json.loads(read_bounded(history, 16 * 1024))
        validate_manifest(old)
        if old["release"] != history.parent.name:
            raise ValueError("published manifest differs from release directory")
        if old["version"] == version and old["release"] != manifest["release"]:
            raise ValueError("published version label cannot be reused for changed files")
    if current:
        old_plants = json.loads((current / "plants.json").read_bytes())
        identities = validate_plants(json.loads(files["plants.json"]))
        if any(identities.get(key) != name for key, name in validate_plants(old_plants).items()):
            raise ValueError("cannot delete or rename a published crop ID")
        old_images = json.loads((current / "metadata.json").read_bytes())["images"]
        if not set(old_images) <= set(json.loads(files["metadata.json"])["images"]):
            raise ValueError("cannot remove published image paths")
    destination = output / "releases" / manifest["release"]
    if destination.exists():
        if any(not (destination / name).is_file() or
               (destination / name).read_bytes() != content for name, content in files.items()):
            raise ValueError("immutable release already exists with different content or label")
    if check:
        if current != destination or not destination.is_dir():
            raise ValueError("source differs from latest; publish explicitly")
        return manifest
    output.mkdir(parents=True, exist_ok=True)
    destination.parent.mkdir(parents=True, exist_ok=True)
    staging = Path(tempfile.mkdtemp(prefix=".prepare-", dir=destination.parent))
    try:
        for name, content in files.items():
            (staging / name).write_bytes(content)
        if not destination.exists():
            os.rename(staging, destination)
        with tempfile.NamedTemporaryFile(prefix=".manifest-", dir=output, delete=False) as stream:
            pointer = Path(stream.name)
            stream.write(files["manifest.json"])
            stream.flush()
            os.fsync(stream.fileno())
        try:
            os.replace(pointer, output / "manifest.json")
        finally:
            pointer.unlink(missing_ok=True)
    finally:
        shutil.rmtree(staging, ignore_errors=True)
    return manifest


def main():
    root = Path(__file__).resolve().parents[1]
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=root / "catalogue/source")
    parser.add_argument("--output", type=Path, default=root / "catalogue")
    parser.add_argument("--version", required=True)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        manifest = prepare(args.source, args.output, args.version, args.check)
    except (OSError, ValueError, KeyError, TypeError, zipfile.BadZipFile) as exc:
        parser.exit(1, f"Catalogue rejected: {exc}\n")
    print(f"Catalogue {'checked' if args.check else 'prepared'}: {manifest['release']}")


if __name__ == "__main__":
    main()
