#!/usr/bin/env python3
"""Prepare an offline, reviewable SMB client channel; never publish anything.

Supply GitHub release JSON (an array of exact release API responses) and local
inputs at INPUT/vVERSION/smb-VERSION.mrpack. Only --version selections are used.
Example: python tools/export_minecraft_channel.py --releases-json releases.json
  --input-dir inputs --version 0.13.0 --version 0.5.0 --current 0.13.0
  --min-version 0.11.0 --output prepared/public --inventory prepared/review.json

Keep the inventory outside the public directory. Publish only its public_files
allowlist after reviewing the exact bytes. Metadata, notes, authentication and
adjacent release assets are never copied. Original mrpack bytes are preserved:
server-only manifest downloads remain referenced but are never client targets.
"""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import re
import stat
import sys
import tempfile
from urllib.parse import unquote, urlsplit
import zipfile


MAX_ARCHIVE = 64 * 1024 * 1024
MAX_FILE = 256 * 1024 * 1024
MAX_TOTAL = 768 * 1024 * 1024
CLIENT_DIRS = {"mods", "config", "resourcepacks", "shaderpacks"}
ROOT_FILES = {"options.txt", "servers.dat"}
DOWNLOAD_HOSTS = {"cdn.modrinth.com", "github.com", "raw.githubusercontent.com",
                  "gitlab.com", "release-assets.githubusercontent.com", "objects.githubusercontent.com"}
OPERATIONS_FILES = {"server.properties", "eula.txt", "ops.json", "whitelist.json",
                    "banned-ips.json", "banned-players.json", "usercache.json"}


class ExportError(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise ExportError(message)


def version(value):
    require(isinstance(value, str) and re.fullmatch(r"(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})\.(0|[1-9][0-9]{0,8})", value),
            "versions must be stable numeric X.Y.Z releases")
    return tuple(map(int, value.split(".")))


def digest(data):
    return hashlib.sha256(data).hexdigest()


def json_bytes(value):
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n").encode("utf-8")


def safe_path(path):
    require(isinstance(path, str) and 0 < len(path) <= 240 and not any(c in path for c in "\\:\0<>\"|?*"),
            "unsafe archive/target path")
    for part in path.split("/"):
        require(part not in {"", ".", ".."} and not part.endswith((".", " "))
                and not any(ord(c) < 32 or ord(c) == 127 for c in part), "unsafe archive/target component")
        stem = part.split(".")[0].upper()
        require(stem not in {"CON", "PRN", "AUX", "NUL", "CLOCK$"}
                and not re.fullmatch(r"(?:COM|LPT)[1-9¹²³]", stem), "unsafe Windows device name")


def client_target(path, directory=False):
    safe_path(path)
    parts = path.split("/")
    require((parts[0] in CLIENT_DIRS and (len(parts) > 1 or directory))
            or (path in ROOT_FILES and not directory), "file is outside allowed client targets")
    for part in parts:
        lower = part.lower()
        require(not lower.startswith((".env", ".git", "docker-compose", "compose."))
                and lower not in OPERATIONS_FILES
                and not lower.endswith((".env", ".pem", ".key")), "server/admin artifact is not publishable")


def public_url(value):
    require(isinstance(value, str) and len(value) <= 4096, "invalid download URL")
    parsed = urlsplit(value)
    require(parsed.scheme == "https" and parsed.hostname in DOWNLOAD_HOSTS
            and parsed.username is None and parsed.password is None and parsed.port in {None, 443},
            "download URL must use an approved public HTTPS source")
    require("jmpijll/smb-minecraft" not in unquote(value).lower(), "private-source download URL is not publishable")


def no_credentials(data):
    require(not re.search(rb"(?:ghp_[A-Za-z0-9]{30,}|github_pat_[A-Za-z0-9_]{30,}|-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----)", data),
            "credential-like content is not publishable")


def inspect_embedded(data, path, budget, depth=0):
    """Audit embedded mods/resources without extracting or altering source bytes."""
    require(depth <= 3, "embedded archive nesting exceeds review limit")
    result = []
    with zipfile.ZipFile(io.BytesIO(data)) as nested:
        require(len(nested.infolist()) <= 10000, "embedded archive has too many entries")
        review = {"path": path, "entries": len(nested.infolist())}
        seen = set()
        for info in nested.infolist():
            require(info.orig_filename == info.filename, "embedded archive path was normalized or truncated")
            name = info.filename[:-1] if info.is_dir() else info.filename
            safe_path(name)
            require(name.casefold() not in seen, "embedded archive has duplicate/case-colliding paths")
            seen.add(name.casefold())
            require(stat.S_IFMT(info.external_attr >> 16) in {0, stat.S_IFDIR, stat.S_IFREG}
                    and not info.flag_bits & 1, "embedded archive has a link, special file or encryption")
            lower = name.lower().split("/")
            require(lower[0] not in {"saves", "world", "world_nether", "world_the_end"}
                    and all(part not in OPERATIONS_FILES and not part.startswith((".env", ".git", "docker-compose", "compose."))
                            and not part.endswith((".pem", ".key", ".env")) for part in lower),
                    "embedded archive contains server/world/admin artifacts")
            require(info.file_size <= MAX_FILE, "embedded file exceeds size limit")
            budget[0] += info.file_size
            require(budget[0] <= MAX_TOTAL, "embedded expanded content exceeds size limit")
            if info.is_dir():
                continue
            payload = nested.read(info)
            no_credentials(payload)
            if name == "fabric.mod.json":
                metadata = json.loads(payload)
                require(isinstance(metadata, dict) and metadata.get("environment", "*") in {"*", "client"},
                        "embedded mod is server-only or has an invalid environment")
                review["mod_environment"] = metadata.get("environment", "*")
                review["mod_contact"] = metadata.get("contact", {})
            if name.lower().endswith((".jar", ".zip")):
                result.extend(inspect_embedded(payload, path + "!/" + name, budget, depth + 1))
        result.append(review)
    return sorted(result, key=lambda item: item["path"])


def inspect_archive(data, expected_version):
    with zipfile.ZipFile(io.BytesIO(data)) as archive:
        infos = archive.infolist()
        require(len(infos) <= 10000, "too many archive entries")
        names = set()
        expanded = 0
        overrides = []
        embedded = []
        nested_budget = [0]
        for info in infos:
            require(info.orig_filename == info.filename, "archive path was normalized or truncated")
            path = info.filename[:-1] if info.is_dir() else info.filename
            safe_path(path)
            require(info.filename.casefold() not in names, "duplicate or case-colliding archive entry")
            names.add(info.filename.casefold())
            mode = stat.S_IFMT(info.external_attr >> 16)
            require(mode in {0, stat.S_IFREG, stat.S_IFDIR} and not info.flag_bits & 1,
                    "archive links, special files or encryption are not permitted")
            require(info.file_size <= MAX_FILE, "archive file exceeds size limit")
            expanded += info.file_size
            require(expanded <= MAX_TOTAL, "expanded archive exceeds size limit")
            if path == "modrinth.index.json":
                require(not info.is_dir() and info.file_size <= 4 * 1024 * 1024, "invalid manifest size/type")
                continue
            if path in {"overrides", "client-overrides"} and info.is_dir():
                continue
            prefix, separator, target = path.partition("/")
            require(separator and prefix in {"overrides", "client-overrides"},
                    "unapproved archive entry (server/source/admin attachments are forbidden)")
            client_target(target, info.is_dir())
            if not info.is_dir():
                payload = archive.read(info)
                no_credentials(payload)
                if info.filename.lower().endswith((".jar", ".zip")):
                    embedded.extend(inspect_embedded(payload, info.filename, nested_budget))
                overrides.append({"path": info.filename, "size": len(payload), "sha256": digest(payload)})
        manifest_bytes = archive.read("modrinth.index.json")
        no_credentials(manifest_bytes)
        manifest = json.loads(manifest_bytes)
        require(isinstance(manifest, dict), "invalid manifest")
        dependencies = manifest.get("dependencies")
        require(type(manifest.get("formatVersion")) is int and manifest["formatVersion"] == 1 and manifest.get("game") == "minecraft"
                and manifest.get("name") == "SMB" and manifest.get("versionId") == expected_version,
                "manifest name/version/game does not match selected stable release")
        require(isinstance(dependencies, dict) and set(dependencies) == {"minecraft", "fabric-loader"},
                "manifest must declare exactly Minecraft and Fabric dependencies")
        for runtime in dependencies.values():
            require(isinstance(runtime, str) and re.fullmatch(r"[A-Za-z0-9._+\-]{1,64}", runtime), "invalid runtime")
        files = manifest.get("files")
        require(isinstance(files, list) and len(files) <= 10000, "invalid manifest file list")
        targets = {}
        ignored = []
        downloads = []
        total = expanded
        for file in files:
            require(isinstance(file, dict), "invalid manifest file")
            path = file.get("path")
            client_target(path)
            require(path.casefold() not in targets, "duplicate/case-colliding manifest targets")
            targets[path.casefold()] = path
            env = file.get("env", {})
            require(isinstance(env, dict), "invalid file environment")
            client = env.get("client", "required")
            require(client in {"required", "optional", "unsupported"}, "unknown client environment")
            urls = file.get("downloads")
            require(isinstance(urls, list) and 0 < len(urls) <= 8, "invalid download sources")
            for url in urls:
                public_url(url)
            sha512 = file.get("hashes", {}).get("sha512")
            require(isinstance(sha512, str) and re.fullmatch(r"[0-9a-f]{128}", sha512), "invalid file SHA512")
            size = file.get("fileSize")
            require(type(size) is int and 0 <= size <= MAX_FILE, "invalid file size")
            if client == "unsupported":
                ignored.append(path)
                continue
            total += size
            require(total <= MAX_TOTAL, "client file total exceeds size limit")
            downloads.append({"path": path, "size": size, "sha512": sha512, "client": client})
        for entry in overrides:
            target = entry["path"].split("/", 1)[1]
            require(targets.get(target.casefold(), target) == target, "case-colliding client targets")
            targets[target.casefold()] = target
        for target in targets:
            components = target.split("/")
            require(all("/".join(components[:i]) not in targets for i in range(1, len(components))),
                    "client file is also used as a directory")
        require(downloads or overrides, "pack has no client files")
        return dependencies, {"overrides": sorted(overrides, key=lambda item: item["path"]),
                              "embedded_archives": sorted(embedded, key=lambda item: item["path"]),
                              "client_downloads": sorted(downloads, key=lambda item: item["path"]),
                              "server_only_downloads_ignored": sorted(ignored)}


def prepare(args):
    current = version(args.current)
    require(version(args.min_version) <= current, "minimum version exceeds current pack")
    selected = args.version
    require(len(selected) <= 1001 and len(set(selected)) == len(selected), "duplicate or excessive version selections")
    require(args.current in selected, "current version must be explicitly selected")
    require(all(version(v) <= current for v in selected), "history cannot be newer than current pack")
    output = Path(args.output).absolute()
    inventory = Path(args.inventory).absolute()
    require(not output.exists() and not output.is_symlink(), "output must be a new directory; refusing overwrite")
    require(not inventory.exists() and not inventory.is_symlink(), "inventory must be a new file; refusing overwrite")
    require(not inventory.resolve().is_relative_to(output.resolve()), "inventory must be outside public payload")
    source = Path(args.input_dir)
    require(source.is_dir() and not source.is_symlink(), "input directory must be a real directory")
    metadata_path = Path(args.releases_json)
    require(metadata_path.stat().st_size <= 16 * 1024 * 1024, "release metadata exceeds size limit")
    releases = json.loads(metadata_path.read_text(encoding="utf-8-sig"))
    require(isinstance(releases, list), "release JSON must be an array of GitHub release objects")
    packs = []
    for selected_version in sorted(selected, key=version, reverse=True):
        tag = "v" + selected_version
        matches = [r for r in releases if isinstance(r, dict) and r.get("tag_name") == tag]
        require(len(matches) == 1, "selected tag must have exactly one release record: " + tag)
        release = matches[0]
        require(release.get("draft") is False and release.get("prerelease") is False,
                "selected release is draft/prerelease or lacks stable evidence: " + tag)
        name = "smb-" + selected_version + ".mrpack"
        assets = release.get("assets")
        require(isinstance(assets, list), "invalid release assets")
        matches = [a for a in assets if isinstance(a, dict) and a.get("name") == name]
        require(len(matches) == 1, "release must contain exactly one named SMB mrpack: " + name)
        asset = matches[0]
        path = source / tag / name
        require(not path.parent.is_symlink() and not path.is_symlink() and path.is_file(), "mrpack must be a local regular file")
        require(0 < path.stat().st_size <= MAX_ARCHIVE, "mrpack exceeds archive size limit")
        data = path.read_bytes()
        require(type(asset.get("size")) is int and asset["size"] == len(data), "release asset size mismatch")
        sha256 = digest(data)
        if asset.get("digest") is not None:
            require(asset["digest"] == "sha256:" + sha256, "release asset SHA256 mismatch")
        dependencies, review = inspect_archive(data, selected_version)
        ref = {"version": selected_version, "min_version": args.min_version if selected_version == args.current else selected_version,
               "url": "packs/" + name, "sha256": sha256, "size": len(data),
               "minecraft": dependencies["minecraft"], "loader": "fabric", "loader_version": dependencies["fabric-loader"]}
        packs.append((ref, data, review))
    channel = {"schema": 1, "pack": next(ref for ref, _, _ in packs if ref["version"] == args.current),
               "history": [ref for ref, _, _ in packs if ref["version"] != args.current]}
    payload = {"stable.json": json_bytes(channel), **{ref["url"]: data for ref, data, _ in packs}}
    review = {"schema": 1, "proposed_destination": "jmpijll/smb-minecraft-client",
              "public_files": [{"path": name, "size": len(data), "sha256": digest(data)} for name, data in sorted(payload.items())],
              "packs": [{"version": ref["version"], **details} for ref, _, details in packs],
              "review_required": "Review exact public_files bytes, including embedded mods/resources, before any publication. No publication performed."}
    return output, inventory, payload, json_bytes(review)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    for name in ["releases-json", "input-dir", "output", "inventory", "current", "min-version"]:
        parser.add_argument("--" + name, required=True)
    parser.add_argument("--version", action="append", required=True, help="exact stable version; repeat for history")
    args = parser.parse_args(argv)
    try:
        output, inventory, payload, review = prepare(args)
        # All source validation precedes any output. Stage on the destination
        # filesystem, then rename; never merge into an existing public directory.
        output.parent.mkdir(parents=True, exist_ok=True)
        inventory.parent.mkdir(parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix=".smb-export-", dir=output.parent) as temporary:
            staged = Path(temporary) / "payload"
            staged.mkdir()
            for name, data in payload.items():
                target = staged / name
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes(data)
            with inventory.open("xb") as handle:
                handle.write(review)
            try:
                require(not output.exists(), "output appeared during preparation; refusing overwrite")
                os.rename(staged, output)
            except Exception:
                inventory.unlink()
                raise
        print("Prepared " + str(len(payload)) + " public files; review inventory: " + str(inventory))
        return 0
    except (ExportError, OSError, ValueError, TypeError, KeyError, AttributeError, zipfile.BadZipFile, RuntimeError) as error:
        print("export: " + str(error), file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
