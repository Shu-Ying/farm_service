"""Offline publication contract; every fixture lives in a temporary directory."""

import hashlib
import copy
import json
from pathlib import Path
import subprocess
import shutil
import sys
import tempfile
import time
import unittest
import zipfile

from PIL import Image

SCRIPT = Path(__file__).resolve().parents[1] / "scripts/prepare_catalogue.py"
sys.path.insert(0, str(SCRIPT.parent))


class PublicationTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.source = self.root / "source"
        self.output = self.root / "output"
        (self.source / "images/plants").mkdir(parents=True)
        Image.new("RGBA", (2, 3), "red").save(
            self.source / "images/plants/carrot.png")
        self.plant = dict(id=1, name="Carrot", level=0, buy=1, isVip=0,
                          vipBuy=0, experience=1, harvest=1, price=2,
                          time=1, crop=1, phase="1,2", general=1,
                          offsetH=0, offsetW=0, offsetX=0, offsetY=0)
        self.save_plants([self.plant])
        self.metadata = {
            "schema": 1, "canvas": [2, 3], "soil_size": [2, 3],
            "positions": [{"x": 1, "y": 1} for _ in range(30)],
            "plants": {}, "images": {},
            "placement": {"schema": 4, "mode": "visible_contact_center",
                "common_scale": 1, "growth_scale": 1,
                "reference_soil_height": 1, "reference_soil_width": 1,
                "contact_alpha_threshold": 32, "contact_band_fraction": 0.2,
                "scene_transform": [[1, 0], [0, 1]]},
            "pest": {"display_size": [2, 3],
                "frames": ["pest/1.png", "pest/2.png", "pest/3.png"],
                "positions": [[1, 1], [1, 2], [2, 1]]},
            "panel": {"reference_canvas": [2, 3], "avatar": [0, 0, 1, 1],
                "fields": [{"field": field, "rect": [0, 0, 1, 1],
                    "color": [0, 0, 0], "alignment": 3} for field in
                    ["name", "experience", "level", "point", "vip_point"]],
                "sprites": [{"sprite": "background/ripe.png", "rect": [0, 0, 1, 1]}]},
            "decorations": {theme: [{"position": [1, 1], "size": [2, 3],
                "layer": "background", "sprite": "background/ripe.png"}]
                for theme in ["default", "spring", "summer", "autumn", "winter", "bee"]},
        }
        self.save_metadata(self.metadata)
        for name in ["webgl/seed.png", "webgl/wither.png", "background/ripe.png",
                     "soil/草土地.png", "soil/普通土地.png", *self.metadata["pest"]["frames"]]:
            path = self.source / "images" / name
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(self.source / "images/plants/carrot.png", path)
        with zipfile.ZipFile(self.source / "land-soils.zip", "w") as archive:
            for name in ("soil/红土地.png", "soil/黑土地.png", "soil/金土地.png"):
                archive.writestr(name, (self.source /
                    "images/plants/carrot.png").read_bytes())

    def save_metadata(self, metadata):
        (self.source / "metadata.json").write_text(json.dumps(metadata), encoding="utf-8")

    def save_plants(self, plants):
        (self.source / "plants.json").write_text(json.dumps({
            "schema": 1, "plants": plants}), encoding="utf-8")

    def run_tool(self, *extra):
        return subprocess.run([sys.executable, str(SCRIPT), "--source",
            str(self.source), "--output", str(self.output), "--version",
            "test-1", *extra], capture_output=True, text=True)

    def publish(self):
        result = self.run_tool()
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads((self.output / "manifest.json").read_bytes())

    def test_release_binds_four_files_and_is_reproducible(self):
        manifest = self.publish()
        self.assertEqual(set(manifest["files"]), {
            "plants.json", "metadata.json", "assets.zip", "land-soils.zip"})
        canonical = json.dumps(manifest["files"], sort_keys=True,
                               separators=(",", ":")).encode()
        self.assertEqual(manifest["release"], hashlib.sha256(canonical).hexdigest())
        release = self.output / "releases" / manifest["release"]
        before = {p.name: p.read_bytes() for p in release.iterdir()}
        self.publish()
        self.assertEqual(before, {p.name: p.read_bytes() for p in release.iterdir()})
        metadata = json.loads(before["metadata.json"])
        self.assertEqual(metadata["plants"]["Carrot"]["phase"], "1,2")
        self.assertEqual(metadata["images"]["plants/carrot.png"]["size"], [2, 3])
        self.assertEqual(self.run_tool("--check").returncode, 0)
        self.plant["price"] = 3
        self.save_plants([self.plant])
        self.assertNotEqual(self.run_tool("--check").returncode, 0)
        self.assertEqual(json.loads((self.output / "manifest.json").read_bytes()), manifest)

    def test_rejects_removed_or_renamed_ids_without_changing_latest(self):
        manifest = self.publish()
        for rows in ([], [dict(self.plant, name="Renamed")]):
            self.save_plants(rows)
            self.assertNotEqual(self.run_tool("--version", "test-2").returncode, 0)
            self.assertEqual(json.loads((self.output / "manifest.json").read_bytes()), manifest)

    def test_rejects_removed_images_corrupt_files_and_duplicate_ids(self):
        manifest = self.publish()
        (self.source / "images/plants/carrot.png").unlink()
        self.assertNotEqual(self.run_tool("--version", "test-2").returncode, 0)
        Image.new("RGB", (1, 1)).save(self.source / "images/plants/carrot.png")
        self.save_plants([self.plant, self.plant])
        self.assertNotEqual(self.run_tool().returncode, 0)
        self.save_plants([self.plant])
        release = self.output / "releases" / manifest["release"]
        (release / "plants.json").write_bytes(b"corrupt")
        self.assertNotEqual(self.run_tool().returncode, 0)

    def test_rejects_unsafe_soil_zip_before_publication(self):
        with zipfile.ZipFile(self.source / "land-soils.zip", "w") as archive:
            archive.writestr("../escape.png", b"bad")
        self.assertNotEqual(self.run_tool().returncode, 0)
        self.assertFalse((self.output / "manifest.json").exists())

    def test_numeric_update_and_new_id_keep_old_release_immutable(self):
        first = self.publish()
        old_release = self.output / "releases" / first["release"]
        old_bytes = (old_release / "plants.json").read_bytes()
        self.plant["price"] = 20
        second_plant = dict(self.plant, id=2, name="Turnip")
        self.save_plants([self.plant, second_plant])
        result = self.run_tool("--version", "test-2")
        self.assertEqual(result.returncode, 0, result.stderr)
        second = json.loads((self.output / "manifest.json").read_bytes())
        self.assertNotEqual(second["release"], first["release"])
        self.assertEqual((old_release / "plants.json").read_bytes(), old_bytes)
        self.assertEqual(self.run_tool("--version", "test-2", "--check").returncode, 0)

    def test_combined_expanded_limit_applies_to_both_archives(self):
        # A valid image twice is permitted individually, but the total is bounded.
        import importlib.util
        spec = importlib.util.spec_from_file_location("prepare_catalogue", SCRIPT)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        picture_size = (self.source / "images/plants/carrot.png").stat().st_size
        module.MAX_EXPANDED = picture_size * 10
        with self.assertRaises(ValueError):
            module.prepare(self.source, self.output, "test-1")

    def test_rejects_values_outside_client_domain_limits(self):
        for changes in ({"price": 2**63}, {"time": 2**63 // 3600 + 1},
                        {"name": "x" * 129}, {"phase": "1,9223372036854775808"}):
            with self.subTest(changes=changes):
                shutil.rmtree(self.output, ignore_errors=True)
                self.save_plants([dict(self.plant, **changes)])
                self.assertNotEqual(self.run_tool().returncode, 0)

    def test_rejects_names_that_cannot_be_safe_crop_asset_components(self):
        for name in (".", "..", "Carrot/Turnip", "Carrot\\Turnip"):
            with self.subTest(name=name):
                shutil.rmtree(self.output, ignore_errors=True)
                self.save_plants([dict(self.plant, name=name)])
                self.assertNotEqual(self.run_tool().returncode, 0)
                self.assertFalse((self.output / "manifest.json").exists())

    def test_oversized_integer_coordinates_raise_validation_error(self):
        from catalogue_validation import numbers
        for value in (10**1000, -(10**1000)):
            with self.subTest(value_sign=value > 0):
                try:
                    numbers([value], 1)
                except Exception as error:
                    self.assertIsInstance(error, ValueError)
                else:
                    self.fail("oversized coordinate was accepted")

    def test_rejects_malformed_metadata_before_publication(self):
        changes = [
            lambda m: m.pop("soil_size"),
            lambda m: m.update(schema=True),
            lambda m: m.update(canvas=[True, 3]),
            lambda m: m.update(positions=[{"x": 1, "y": 1}]),
            lambda m: m["placement"].update(schema=True),
            lambda m: m["placement"].update(common_scale=float("nan")),
            lambda m: m["placement"].update(growth_scale=-1),
            lambda m: m["placement"].update(scene_transform=[[1, 1], [1, 1]]),
            lambda m: m["pest"].update(frames=["pest/1.png"] * 3),
            lambda m: m["pest"].update(display_size=[8193, 1]),
            lambda m: m["panel"].update(fields=[]),
            lambda m: m["panel"]["fields"][0].update(color=[0, float("inf"), 0]),
            lambda m: m["panel"]["fields"][0].update(alignment=True),
            lambda m: m["panel"]["sprites"][0].update(rect=[0, 0, 0, 1]),
            lambda m: m["decorations"].pop("winter"),
            lambda m: m["decorations"]["winter"][0].update(layer="unknown"),
        ]
        for change in changes:
            with self.subTest(change=change):
                shutil.rmtree(self.output, ignore_errors=True)
                metadata = copy.deepcopy(self.metadata)
                change(metadata)
                self.save_metadata(metadata)
                self.assertNotEqual(self.run_tool().returncode, 0)
                self.assertFalse((self.output / "manifest.json").exists())

    def test_rejects_required_missing_sprite_references(self):
        for kind in ("required", "pest", "panel", "decoration"):
            with self.subTest(kind=kind):
                shutil.rmtree(self.output, ignore_errors=True)
                metadata = copy.deepcopy(self.metadata)
                required = self.source / "images/webgl/seed.png"
                if kind == "required":
                    content = required.read_bytes()
                    required.unlink()
                elif kind == "pest":
                    metadata["pest"]["frames"][0] = "missing.png"
                elif kind == "panel":
                    metadata["panel"]["sprites"][0]["sprite"] = "missing.png"
                else:
                    metadata["decorations"]["winter"][0]["sprite"] = "missing.png"
                self.save_metadata(metadata)
                self.assertNotEqual(self.run_tool().returncode, 0)
                if kind == "required":
                    required.write_bytes(content)

    def test_rejects_incomplete_soil_bundle(self):
        with zipfile.ZipFile(self.source / "land-soils.zip", "w") as archive:
            archive.writestr("soil/红土地.png", (self.source /
                "images/plants/carrot.png").read_bytes())
        self.assertNotEqual(self.run_tool().returncode, 0)

    def test_rejects_reusing_version_label_for_changed_release(self):
        first = self.publish()
        self.plant["price"] = 30
        self.save_plants([self.plant])
        self.assertNotEqual(self.run_tool().returncode, 0)
        self.assertEqual(json.loads((self.output / "manifest.json").read_bytes()), first)

    def paused_publisher(self, source, version):
        ready, proceed = self.root / "ready", self.root / "proceed"
        code = """
import sys, time
from pathlib import Path
sys.path.insert(0, sys.argv[1])
import prepare_catalogue as module
original = module.build
def paused(*args):
    Path(sys.argv[5]).touch()
    deadline = time.monotonic() + 20
    while not Path(sys.argv[6]).exists():
        if time.monotonic() > deadline:
            raise RuntimeError('test barrier timed out')
        time.sleep(0.01)
    return original(*args)
module.build = paused
module.prepare(Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4])
"""
        process = subprocess.Popen([sys.executable, "-c", code,
            str(SCRIPT.parent), str(source), str(self.output), version,
            str(ready), str(proceed)], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: process.kill() if process.poll() is None else None)
        deadline = time.monotonic() + 10
        while not ready.exists() and time.monotonic() < deadline:
            self.assertIsNone(process.poll())
            time.sleep(0.01)
        self.assertTrue(ready.exists())
        return process, proceed

    def test_cross_process_publishers_cannot_erase_concurrently_added_ids(self):
        self.publish()
        newer = self.root / "newer"
        shutil.copytree(self.source, newer)
        (newer / "plants.json").write_text(json.dumps({"schema": 1, "plants": [
            self.plant, dict(self.plant, id=2, name="Turnip")]}), encoding="utf-8")
        self.plant["price"] = 50
        self.save_plants([self.plant])
        old, proceed = self.paused_publisher(self.source, "test-old-writer")
        added = subprocess.Popen([sys.executable, str(SCRIPT), "--source", str(newer),
            "--output", str(self.output), "--version", "test-new-writer"],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.addCleanup(lambda: added.kill() if added.poll() is None else None)
        try:
            added.wait(timeout=1.5)
        except subprocess.TimeoutExpired:
            pass
        proceed.touch()
        old_out, old_error = old.communicate(timeout=10)
        added_out, added_error = added.communicate(timeout=10)
        self.assertEqual(old.returncode, 0, (old_out, old_error))
        self.assertEqual(added.returncode, 0, (added_out, added_error))
        latest = json.loads((self.output / "manifest.json").read_bytes())
        rows = json.loads((self.output / "releases" / latest["release"] /
                           "plants.json").read_bytes())["plants"]
        self.assertEqual({row["id"] for row in rows}, {1, 2})

    def test_process_exit_releases_publication_lock(self):
        self.publish()
        process, _ = self.paused_publisher(self.source, "test-interrupted")
        process.kill()
        process.communicate(timeout=10)
        self.assertEqual(self.run_tool().returncode, 0)

    def test_threads_publish_without_losing_new_ids(self):
        from concurrent.futures import ThreadPoolExecutor, TimeoutError
        import threading
        from unittest.mock import patch
        import prepare_catalogue as module

        self.publish()
        newer = self.root / "newer"
        shutil.copytree(self.source, newer)
        (newer / "plants.json").write_text(json.dumps({"schema": 1, "plants": [
            self.plant, dict(self.plant, id=2, name="Turnip")]}), encoding="utf-8")
        self.plant["price"] = 50
        self.save_plants([self.plant])
        ready, proceed = threading.Event(), threading.Event()
        original = module.build

        def paused(source, version):
            if version == "old-thread":
                ready.set()
                if not proceed.wait(10):
                    raise RuntimeError("test barrier timed out")
            return original(source, version)

        with patch.object(module, "build", paused), ThreadPoolExecutor(2) as executor:
            old = executor.submit(module.prepare, self.source, self.output, "old-thread")
            self.assertTrue(ready.wait(10))
            added = executor.submit(module.prepare, newer, self.output, "new-thread")
            try:
                added.result(timeout=0.5)
            except TimeoutError:
                pass
            finally:
                proceed.set()
            old.result(timeout=10)
            latest = added.result(timeout=10)
        rows = json.loads((self.output / "releases" / latest["release"] /
                           "plants.json").read_bytes())["plants"]
        self.assertEqual({row["id"] for row in rows}, {1, 2})

    def test_check_does_not_create_or_modify_output_files(self):
        self.assertNotEqual(self.run_tool("--check").returncode, 0)
        self.assertFalse(self.output.exists())
        self.publish()
        before = {p.relative_to(self.output): (p.read_bytes(), p.stat().st_mtime_ns)
                  for p in self.output.rglob("*") if p.is_file()}
        self.assertEqual(self.run_tool("--check").returncode, 0)
        after = {p.relative_to(self.output): (p.read_bytes(), p.stat().st_mtime_ns)
                 for p in self.output.rglob("*") if p.is_file()}
        self.assertEqual(after, before)

    def test_interrupted_staging_manifest_does_not_reserve_version_label(self):
        first = self.publish()
        orphan = self.output / "releases/.prepare-interrupted"
        orphan.mkdir()
        orphan_manifest = dict(first, version="test-2")
        (orphan / "manifest.json").write_text(json.dumps(orphan_manifest), encoding="utf-8")
        self.plant["price"] = 25
        self.save_plants([self.plant])
        result = self.run_tool("--version", "test-2")
        self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == "__main__":
    unittest.main()
