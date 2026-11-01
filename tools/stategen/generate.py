"""Fetch verified vanilla inputs and run StateDump or SoundDump on Linux or Windows."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import urllib.request
import zipfile

MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
ART_URL = (
    "https://maven.neoforged.net/releases/net/neoforged/AutoRenamingTool/2.0.17/"
    "AutoRenamingTool-2.0.17-all.jar"
)
ART_SHA256 = "e977ee4422efb51eb54cc99f939833cef769de8b4aa1043294ce5400c60f3b1f"


def fetch(url, path, digest, algorithm="sha1"):
    """Cache a download only after its published or pinned digest matches."""
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists() and hashlib.new(algorithm, path.read_bytes()).hexdigest() == digest:
        return
    print(f"Downloading {url}", flush=True)
    with urllib.request.urlopen(url) as response:
        data = response.read()
    actual = hashlib.new(algorithm, data).hexdigest()
    if actual != digest:
        raise ValueError(f"{url}: {algorithm} {actual}, expected {digest}")
    path.write_bytes(data)


def prepare(version, java):
    """Verify Mojang inputs, unpack the bundle, and reverse official mappings."""
    ref = Path("reference") / version
    with urllib.request.urlopen(MANIFEST) as response:
        manifest = json.load(response)
    entry = next(v for v in manifest["versions"] if v["id"] == version)
    metadata = ref / "version.json"
    fetch(entry["url"], metadata, entry["sha1"])
    downloads = json.loads(metadata.read_text())["downloads"]
    server = downloads["server"]
    jar = ref / "server.jar"
    fetch(server["url"], jar, server["sha1"])
    with zipfile.ZipFile(jar) as bundle:
        for name in bundle.namelist():
            if name.startswith(("META-INF/versions/", "META-INF/libraries/")) and name.endswith(".jar"):
                target = ref / "bundler" / name
                data = bundle.read(name)
                if not target.exists() or target.read_bytes() != data:
                    target.parent.mkdir(parents=True, exist_ok=True)
                    target.write_bytes(data)
    classes = list((ref / "bundler/META-INF/versions").rglob("*.jar"))
    if len(classes) != 1:
        raise ValueError(f"{version}: expected one server classes jar, found {classes}")
    classes = classes[0]
    libraries = sorted((ref / "bundler/META-INF/libraries").rglob("*.jar"))
    if "server_mappings" in downloads:
        mapping = downloads["server_mappings"]
        mappings = ref / "server-mappings.txt"
        fetch(mapping["url"], mappings, mapping["sha1"])
        remapper = Path("reference/tools/AutoRenamingTool-2.0.17-all.jar")
        fetch(ART_URL, remapper, ART_SHA256, "sha256")
        deobf = ref / f"server-{version}-deobf.jar"
        # Official ProGuard mappings run named -> obfuscated; --reverse makes
        # ART translate the extracted obfuscated server back to Mojang names.
        if not deobf.exists() or deobf.stat().st_mtime < max(classes.stat().st_mtime, mappings.stat().st_mtime):
            command = [java, "-Xmx2G", "-jar", str(remapper), "--input", str(classes),
                       "--output", str(deobf), "--map", str(mappings), "--reverse", "--strip-sigs",
                       "--threads", "6"]
            for library in libraries:
                command.extend(["--lib", str(library)])
            subprocess.run(command, check=True)
        classes = deobf
    return ref, classes, libraries


def compile_tool(javac, name):
    """Compile one reflection-only dumper into tools/stategen/out."""
    out = Path("tools/stategen/out")
    out.mkdir(parents=True, exist_ok=True)
    subprocess.run([javac, "--release", "21", "-d", str(out), f"tools/stategen/{name}.java"], check=True)
    return out


def classpath(version, java, out):
    """The verified server classes, their bundled libraries and the dumpers."""
    _, classes, libraries = prepare(version, java)
    return os.pathsep.join(map(str, [classes, *libraries, out]))


def state(version, java, javac):
    """Dump one version's per-state table and compact it with blockgen."""
    out = compile_tool(javac, "StateDump")
    ref = Path("reference") / version
    generated = ref / "generated/state.json"
    generated.parent.mkdir(parents=True, exist_ok=True)
    data = Path("pomme-client/src/world/block/data")
    subprocess.run([java, "-Xmx2G", "-cp", classpath(version, java, out), "StateDump", version,
                    str(generated), str(data / f"piston-{version}.json")], check=True)
    subprocess.run(["cargo", "run", "-p", "blockgen", "--", "state", str(generated),
                    str(data / f"blocks-{version}.json"),
                    str(data / f"state-{version}.json")], check=True)


def sounds(java, javac):
    """Dump every version with a state table, newest first, and merge the dumps."""
    out = compile_tool(javac, "SoundDump")
    data = Path("pomme-client/src/world/block/data")
    versions = sorted((p.name[len("state-"):-len(".json")] for p in data.glob("state-*.json")),
                      key=lambda v: tuple(int(part) for part in v.split(".")), reverse=True)
    dumps = []
    for version in versions:
        dump = Path("reference") / version / "generated/sounds.json"
        dump.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run([java, "-cp", classpath(version, java, out), "SoundDump", str(dump)], check=True)
        dumps.append(f"{version}={dump}")
    subprocess.run(["cargo", "run", "-p", "blockgen", "--", "sounds",
                    "pomme-client/src/world/block/block_sounds.json", *dumps], check=True)


def main():
    """Run StateDump for one version, or SoundDump for every version."""
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    state_parser = commands.add_parser("state")
    state_parser.add_argument("version")
    sounds_parser = commands.add_parser("sounds")
    for command in (state_parser, sounds_parser):
        command.add_argument("--jdk", required=True, help="JDK bin directory (25+ for 26.x)")
    args = parser.parse_args()
    suffix = ".exe" if os.name == "nt" else ""
    java = str(Path(args.jdk) / f"java{suffix}")
    javac = str(Path(args.jdk) / f"javac{suffix}")
    if args.command == "state":
        state(args.version, java, javac)
    else:
        sounds(java, javac)


if __name__ == "__main__":
    main()
