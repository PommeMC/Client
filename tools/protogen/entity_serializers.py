#!/usr/bin/env python3
"""Extract EntityDataSerializers registration order from verified vanilla jars.

Usage: python3 tools/protogen/entity_serializers.py <reference-root> <version> <out.json>
Downloads official server jars and mappings when absent, verifies SHA-1, and
uses javap to read registration calls. No game code is executed. Requires a JDK.
Reference downloads, mappings, and javap output stay under <reference-root>.
"""

import hashlib
import io
import json
from pathlib import Path
import re
import subprocess
import sys
import urllib.request
import zipfile

MANIFEST = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json"
CLASS = "net.minecraft.network.syncher.EntityDataSerializers"


def download(root, name, descriptor):
    """Return an official artifact only after verifying its advertised SHA-1."""
    path = root / name
    if not path.exists() or hashlib.sha1(path.read_bytes()).hexdigest() != descriptor["sha1"]:
        urllib.request.urlretrieve(descriptor["url"], path)
    if hashlib.sha1(path.read_bytes()).hexdigest() != descriptor["sha1"]:
        raise ValueError(f"SHA-1 mismatch: {path}")
    return path


def mapped_members(mapping):
    """Resolve the serializer class, constant names, and registration method."""
    section = mapping.split(CLASS + " -> ", 1)[1]
    obfuscated, section = section.split(":", 1)
    lines = []
    for line in section.splitlines()[1:]:
        if line and not line.startswith((" ", "#")):
            break
        lines.append(line.strip())
    fields = {}
    register = None
    for line in lines:
        if " -> " not in line:
            continue
        original, renamed = line.rsplit(" -> ", 1)
        if "(" not in original and "EntityDataSerializer " in original:
            fields[renamed] = original.split()[-1]
        if " registerSerializer(" in original:
            register = renamed
    if not fields or register is None:
        raise ValueError("incomplete EntityDataSerializers mapping")
    return obfuscated, fields, register


def registrations(bytecode, fields, register):
    """Read static-block getstatic/register pairs, rejecting unresolved fields."""
    block = bytecode.split("static {};", 1)[1]
    names = []
    previous = ""
    for line in block.splitlines():
        if re.search(r"\binvokestatic\b.*// Method " + re.escape(register) + r":", line):
            match = re.search(r"\bgetstatic\b.*// Field ([\w$]+):", previous)
            if match is None:
                raise ValueError(f"registration without a constant: {line}")
            field = match[1]
            names.append(fields[field] if fields is not None else field)
        if re.match(r"\s*\d+:", line):
            previous = line
    if not names or len(set(names)) != len(names):
        raise ValueError("empty or duplicate serializer registrations")
    return [name.lower() for name in names]


def generate(reference, version, output):
    """Generate ordered names and artifact provenance for one protocol."""
    root = reference / version
    root.mkdir(parents=True, exist_ok=True)
    metadata_path = root / "version.json"
    if metadata_path.exists():
        metadata = json.loads(metadata_path.read_text())
    else:
        manifest = json.load(urllib.request.urlopen(MANIFEST, timeout=60))
        entry = next(entry for entry in manifest["versions"] if entry["id"] == version)
        metadata = json.load(urllib.request.urlopen(entry["url"], timeout=60))
        metadata_path.write_text(json.dumps(metadata, indent=2) + "\n")
    if metadata["id"] != version:
        raise ValueError("cached version metadata does not match requested version")
    descriptors = metadata["downloads"]
    jar = download(root, "server.jar", descriptors["server"])
    fields = None
    class_name = CLASS
    register = "registerSerializer"
    if "server_mappings" in descriptors:
        mapping = download(root, "server.txt", descriptors["server_mappings"])
        class_name, fields, register = mapped_members(mapping.read_text())
    with zipfile.ZipFile(jar) as bundle:
        paths = [name for name in bundle.namelist()
                 if name.startswith("META-INF/versions/") and name.endswith(".jar")]
        if len(paths) != 1:
            raise ValueError("expected one bundled server jar")
        with zipfile.ZipFile(io.BytesIO(bundle.read(paths[0]))) as server:
            class_file = root / "EntityDataSerializers.class"
            class_file.write_bytes(server.read(class_name.replace(".", "/") + ".class"))
    bytecode = subprocess.check_output(["javap", "-c", "-p", str(class_file)], text=True)
    (root / "EntityDataSerializers.javap").write_text(bytecode)
    # version.json inside the bundled jar is Mojang's own protocol authority.
    with zipfile.ZipFile(jar) as bundle:
        with zipfile.ZipFile(io.BytesIO(bundle.read(paths[0]))) as server:
            protocol = json.loads(server.read("version.json"))["protocol_version"]
    result = {
        "version": version,
        "protocol": protocol,
        "server_sha1": descriptors["server"]["sha1"],
        "mappings_sha1": descriptors.get("server_mappings", {}).get("sha1"),
        "serializers": registrations(bytecode, fields, register),
    }
    output.write_text(json.dumps(result, indent=2) + "\n")
    print(f"{version}: {len(result['serializers'])} serializers")


if __name__ == "__main__":
    if len(sys.argv) != 4:
        sys.exit(__doc__)
    generate(Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3]))
