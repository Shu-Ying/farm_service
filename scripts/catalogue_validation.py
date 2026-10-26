"""Independent Service-side render contract; never imports a Farm checkout."""

import math
from pathlib import PurePosixPath

SOIL_MEMBERS = {"soil/红土地.png", "soil/黑土地.png", "soil/金土地.png"}
REQUIRED_IMAGES = {"webgl/seed.png", "webgl/wither.png", "background/ripe.png",
                   "soil/草土地.png", "soil/普通土地.png"}


def numbers(values, length):
    if (type(values) is not list or len(values) != length
            or any(type(v) not in (int, float) or abs(v) > 10000000
                   or not math.isfinite(v) for v in values)):
        raise ValueError("invalid render coordinates")
    return values


def image_size(values):
    numbers(values, 2)
    if (any(type(v) is not int or not 0 < v <= 8192 for v in values)
            or values[0] * values[1] > 32 * 1024 * 1024):
        raise ValueError("invalid image dimensions")


def rectangle(values):
    numbers(values, 4)
    if values[2] <= 0 or values[3] <= 0:
        raise ValueError("invalid render rectangle")


def safe_member(name):
    if (not isinstance(name, str) or not name or len(name) > 512
            or "\\" in name or ":" in name):
        raise ValueError("invalid image path")
    path = PurePosixPath(name)
    if path.is_absolute() or any(p in ("", ".", "..") for p in name.split("/")):
        raise ValueError("invalid image path")
    return name


def validate_metadata(value, rows):
    """Validate every renderer-required field; keep legacy phase placeholders."""
    try:
        if type(value["schema"]) is not int or value["schema"] != 1:
            raise ValueError("unknown render metadata schema")
        image_size(value["canvas"])
        image_size(value["soil_size"])
        if type(value["positions"]) is not list or len(value["positions"]) != 30:
            raise ValueError("expected 30 plot positions")
        for position in value["positions"]:
            numbers([position["x"], position["y"]], 2)
        images = value["images"]
        if type(images) is not dict or not 0 < len(images) <= 10000:
            raise ValueError("invalid image manifest")
        for name, item in images.items():
            safe_member(name)
            image_size(item["size"])
            checksum = item["sha256"]
            if (not isinstance(checksum, str) or len(checksum) != 64
                    or any(c not in "0123456789abcdef" for c in checksum)):
                raise ValueError("invalid image checksum")
        plants = value["plants"]
        if type(plants) is not dict or set(plants) != {row["name"] for row in rows}:
            raise ValueError("catalogue and render plants differ")
        for row in rows:
            if plants[row["name"]]["phase"] != row["phase"]:
                raise ValueError("catalogue and render phases differ")
        placement = value["placement"]
        if (type(placement["schema"]) is not int or placement["schema"] != 4
                or placement["mode"] != "visible_contact_center"):
            raise ValueError("unknown crop placement")
        for key in ("common_scale", "growth_scale", "reference_soil_height",
                    "reference_soil_width"):
            numbers([placement[key]], 1)
            if placement[key] <= 0:
                raise ValueError("invalid crop scale")
        numbers([placement["contact_alpha_threshold"]], 1)
        numbers([placement["contact_band_fraction"]], 1)
        if (not 0 <= placement["contact_alpha_threshold"] <= 255
                or not 0 < placement["contact_band_fraction"] <= 1):
            raise ValueError("invalid ground contact rule")
        matrix = placement["scene_transform"]
        if type(matrix) is not list or len(matrix) != 2:
            raise ValueError("invalid scene transform")
        a, b = numbers(matrix[0], 2)
        c, d = numbers(matrix[1], 2)
        if abs(a * d - b * c) < 1e-8:
            raise ValueError("degenerate scene transform")
        pest = value["pest"]
        image_size(pest["display_size"])
        frames = pest["frames"]
        if (type(frames) is not list or len(frames) < 3
                or len(set(frames)) != len(frames)):
            raise ValueError("invalid pest frames")
        if any(name not in images for name in frames):
            raise ValueError("missing pest image")
        if type(pest["positions"]) is not list or len(pest["positions"]) < 3:
            raise ValueError("missing pest positions")
        for position in pest["positions"]:
            numbers(position, 2)
        panel = value["panel"]
        image_size(panel["reference_canvas"])
        rectangle(panel["avatar"])
        fields = {"name", "experience", "level", "point", "vip_point"}
        if (type(panel["fields"]) is not list
                or {entry["field"] for entry in panel["fields"]} != fields):
            raise ValueError("invalid panel fields")
        for entry in panel["fields"]:
            rectangle(entry["rect"])
            numbers(entry["color"], 3)
            if (any(not 0 <= v <= 255 for v in entry["color"])
                    or type(entry["alignment"]) is not int
                    or entry["alignment"] not in (3, 4)):
                raise ValueError("invalid panel text")
        if type(panel["sprites"]) is not list:
            raise ValueError("invalid panel sprites")
        for entry in panel["sprites"]:
            rectangle(entry["rect"])
            if entry["sprite"] not in images:
                raise ValueError("missing panel sprite")
        if set(value["decorations"]) != {"default", "spring", "summer",
                                         "autumn", "winter", "bee"}:
            raise ValueError("invalid decoration themes")
        for entries in value["decorations"].values():
            if type(entries) is not list:
                raise ValueError("invalid decorations")
            for entry in entries:
                numbers(entry["position"], 2)
                numbers(entry["size"], 2)
                if (min(entry["size"]) <= 0
                        or entry["layer"] not in ("background", "foreground")
                        or entry["sprite"] not in images):
                    raise ValueError("invalid decoration")
        if not REQUIRED_IMAGES <= set(images):
            raise ValueError("missing required farm sprite")
    except (KeyError, TypeError, IndexError, AttributeError) as error:
        raise ValueError("invalid render metadata") from error
