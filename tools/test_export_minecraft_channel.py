"""Behavioral fixtures for the author-side, offline Minecraft channel exporter."""
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import zipfile


SCRIPT = Path(__file__).with_name("export_minecraft_channel.py")


class ExportTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name)
        self.source = self.base / "source"
        self.source.mkdir()
        self.output = self.base / "public"
        self.inventory = self.base / "review.json"
        self.releases = []
        self.add_pack("0.13.0")
        self.add_pack("0.5.0")

    def add_pack(self, version, manifest_change=None, entries=None):
        manifest = {
            "formatVersion": 1, "game": "minecraft", "name": "SMB", "versionId": version,
            "dependencies": {"minecraft": "26.3", "fabric-loader": "0.20.1"},
            "files": [
                {"path": "mods/client.jar", "hashes": {"sha512": "a" * 128},
                 "downloads": ["https://cdn.modrinth.com/client.jar"], "fileSize": 20},
                {"path": "mods/server.jar", "hashes": {"sha512": "b" * 128},
                 "downloads": ["https://cdn.modrinth.com/server.jar"], "fileSize": 30,
                 "env": {"client": "unsupported", "server": "required"}},
            ],
        }
        if manifest_change:
            manifest_change(manifest)
        path = self.source / ("v" + version) / ("smb-" + version + ".mrpack")
        path.parent.mkdir(exist_ok=True)
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr("modrinth.index.json", json.dumps(manifest))
            archive.writestr("overrides/config/client.json", '{"enabled":true}')
            for name, value in entries or []:
                # ZipInfo normally normalizes Windows backslashes. Preserve
                # hostile raw names so the exporter, not fixture construction,
                # exercises the path check.
                if isinstance(name, str):
                    raw_name = name
                    name = zipfile.ZipInfo(name)
                    name.filename = raw_name
                archive.writestr(name, value)
        release = {"tag_name": "v" + version, "draft": False, "prerelease": False,
                   "body": "PRIVATE AUTHOR NOTES MUST NOT SHIP",
                   "assets": [{"name": path.name, "size": path.stat().st_size,
                               "digest": "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest()},
                              {"name": "docker-compose.yml", "size": 123}]}
        self.releases = [r for r in self.releases if r["tag_name"] != release["tag_name"]] + [release]
        return manifest

    def run_export(self, *extra):
        metadata = self.base / "releases.json"
        metadata.write_text(json.dumps(self.releases), encoding="utf-8")
        return subprocess.run([sys.executable, str(SCRIPT), "--releases-json", str(metadata),
                               "--input-dir", str(self.source), "--output", str(self.output),
                               "--inventory", str(self.inventory), "--current", "0.13.0",
                               "--min-version", "0.11.0", "--version", "0.13.0",
                               "--version", "0.5.0", *extra], capture_output=True, text=True)

    def assert_rejected(self, *extra):
        result = self.run_export(*extra)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("export:", result.stderr)
        self.assertFalse(self.output.exists(), result.stderr)
        self.assertFalse(self.inventory.exists())

    def test_exact_stable_archives_and_per_version_runtime_are_exported(self):
        self.add_pack("0.5.0", lambda m: m["dependencies"].update(minecraft="1.21.11"))
        result = self.run_export()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(sorted(str(p.relative_to(self.output)).replace("\\", "/")
                                for p in self.output.rglob("*") if p.is_file()),
                         ["packs/smb-0.13.0.mrpack", "packs/smb-0.5.0.mrpack", "stable.json"])
        channel = json.loads((self.output / "stable.json").read_text())
        self.assertEqual(channel["schema"], 1)
        self.assertEqual(channel["pack"]["min_version"], "0.11.0")
        self.assertEqual(channel["history"][0]["minecraft"], "1.21.11")
        self.assertEqual(channel["history"][0]["min_version"], "0.5.0")
        self.assertEqual(channel["pack"]["loader"], "fabric")
        for ref in [channel["pack"], *channel["history"]]:
            original = self.source / ("v" + ref["version"]) / Path(ref["url"]).name
            public = self.output / ref["url"]
            self.assertEqual(public.read_bytes(), original.read_bytes())
            self.assertEqual(ref["size"], public.stat().st_size)
            self.assertEqual(ref["sha256"], hashlib.sha256(public.read_bytes()).hexdigest())
        self.assertNotIn("PRIVATE", (self.output / "stable.json").read_text())
        review = json.loads(self.inventory.read_text())
        self.assertEqual(len(review["public_files"]), 3)
        self.assertEqual(review["packs"][0]["server_only_downloads_ignored"], ["mods/server.jar"])

    def test_repeat_exports_are_byte_deterministic_and_never_overwrite(self):
        first = self.run_export()
        self.assertEqual(first.returncode, 0, first.stderr)
        before = {str(p.relative_to(self.output)): p.read_bytes() for p in self.output.rglob("*") if p.is_file()}
        saved_inventory = self.inventory.read_bytes()
        second_output = self.base / "public2"
        second_inventory = self.base / "review2.json"
        second = self.run_export("--output", str(second_output), "--inventory", str(second_inventory))
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertEqual(before, {str(p.relative_to(second_output)): p.read_bytes() for p in second_output.rglob("*") if p.is_file()})
        self.assertEqual(saved_inventory, second_inventory.read_bytes())
        self.assertNotEqual(self.run_export().returncode, 0)
        self.assertEqual(before, {str(p.relative_to(self.output)): p.read_bytes() for p in self.output.rglob("*") if p.is_file()})

    def test_draft_or_prerelease_selected_releases_are_refused(self):
        for flag in ["draft", "prerelease"]:
            with self.subTest(flag=flag):
                self.releases[0][flag] = True
                self.assert_rejected()
                self.releases[0][flag] = False

    def test_only_explicit_stable_release_selection_is_used(self):
        self.releases.append({"tag_name": "v0.14.0-dev", "draft": False, "prerelease": True})
        result = self.run_export()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(len(list((self.output / "packs").iterdir())), 2)

    def test_unstable_versions_or_impossible_minimum_fail_before_writes(self):
        for args in [("--current", "0.13.0-dev"), ("--min-version", "01.2.3"),
                     ("--min-version", "0.14.0"), ("--version", "0.14.0"),
                     ("--version", "0.13.0")]:
            with self.subTest(args=args):
                self.assert_rejected(*args)

    def test_release_size_hash_name_or_duplicate_tag_mismatch_is_refused(self):
        for field, value in [("size", 1), ("digest", "sha256:" + "0" * 64), ("name", "client.mrpack")]:
            with self.subTest(field=field):
                old = self.releases[0]["assets"][0][field]
                self.releases[0]["assets"][0][field] = value
                self.assert_rejected()
                self.releases[0]["assets"][0][field] = old
        self.releases.append(dict(self.releases[0]))
        self.assert_rejected()

    def test_manifest_identity_loader_and_private_downloads_are_refused(self):
        changes = [lambda m: m.update(name="Other"), lambda m: m.update(versionId="0.12.0"),
                   lambda m: m["dependencies"].update({"forge": "1"}),
                   lambda m: m["files"][0].update(downloads=["https://github.com/jmpijll/smb-minecraft/releases/download/private.jar"]),
                   lambda m: m["files"][0].update(downloads=["http://cdn.modrinth.com/client.jar"]),
                   lambda m: m["files"][0].update(path="saves/world.dat"),
                   lambda m: m["files"][0].update(hashes={"sha512": "bad"})]
        for change in changes:
            with self.subTest(change=change):
                self.add_pack("0.13.0", change)
                self.assert_rejected()

    def test_archive_unsafe_paths_server_and_admin_artifacts_are_refused(self):
        for path in ["overrides/../escape", "overrides/config/C:/bad", "overrides/mods/CON.jar",
                     "overrides/config/a\\b", "overrides/config/test.json:stream", "/absolute",
                     "server-overrides/mods/server.jar", "docker-compose.yml", "overrides/.env",
                     "overrides/config/server.properties", "overrides/config/CLIENT.json",
                     "overrides/options.txt/child", "overrides/config/secrets.env",
                     "overrides/mods/client.jar/child", "overrides/config/client.json/child"]:
            with self.subTest(path=path):
                self.add_pack("0.13.0", entries=[(path, b"bad")])
                self.assert_rejected()

    def test_symlink_zip_entries_are_refused(self):
        info = zipfile.ZipInfo("overrides/mods/link.jar")
        info.create_system = 3
        info.external_attr = 0o120777 << 16
        self.add_pack("0.13.0", entries=[(info, b"../../secret")])
        self.assert_rejected()

    def test_inventory_inside_public_payload_is_refused(self):
        self.assert_rejected("--inventory", str(self.output / "review.json"))

    def test_embedded_shared_mod_metadata_is_disclosed_without_rewriting(self):
        stream = io.BytesIO()
        with zipfile.ZipFile(stream, "w") as nested:
            nested.writestr("fabric.mod.json", json.dumps({"id": "shared", "environment": "*",
                              "contact": {"sources": "https://github.com/jmpijll/smb-minecraft"}}))
        self.add_pack("0.13.0", entries=[("overrides/mods/shared.jar", stream.getvalue())])
        result = self.run_export()
        self.assertEqual(result.returncode, 0, result.stderr)
        pack_review = json.loads(self.inventory.read_text())["packs"][0]
        self.assertEqual(pack_review["embedded_archives"][0]["mod_environment"], "*")
        self.assertEqual(pack_review["embedded_archives"][0]["mod_contact"],
                         {"sources": "https://github.com/jmpijll/smb-minecraft"})

    def test_embedded_tokens_operations_world_and_server_only_mod_are_refused(self):
        cases = [("fabric.mod.json", json.dumps({"environment": "server"}).encode()),
                 ("server.properties", b"motd=server"), ("saves/world/level.dat", b"world"),
                 ("secret.txt", b"ghp_" + b"a" * 36)]
        for name, data in cases:
            with self.subTest(name=name):
                stream = io.BytesIO()
                with zipfile.ZipFile(stream, "w") as nested:
                    nested.writestr(name, data)
                self.add_pack("0.13.0", entries=[("overrides/mods/shared.jar", stream.getvalue())])
                self.assert_rejected()


if __name__ == "__main__":
    unittest.main()
