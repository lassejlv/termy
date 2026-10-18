#!/usr/bin/env python3
"""Sparkle appcast helpers for the release workflow.

  sign  (macOS) EdDSA-sign a release DMG with Sparkle's sign_update and record
        what its appcast item needs (signature, length, bundle versions).
  feed  Turn those records into one appcast-<arch>.xml per architecture.

The app reads https://github.com/lassejlv/termy/releases/latest/download/appcast-<arch>.xml
(SUFeedURL, set by scripts/build-dmg.sh), so each release carries a feed that
lists itself.
"""

import argparse
import email.utils
import json
import os
import plistlib
import re
import subprocess
import sys
import xml.etree.ElementTree as ET
from pathlib import Path

SPARKLE_NS = "http://www.andymatuschak.org/xml-namespaces/sparkle"


def sign(args: argparse.Namespace) -> None:
    key = os.environ.get("SPARKLE_ED_PRIVATE_KEY", "").strip()
    if not key:
        sys.exit("SPARKLE_ED_PRIVATE_KEY is not set")
    output = subprocess.run(
        [args.sign_update, "--ed-key-file", "-", args.dmg],
        input=key,
        capture_output=True,
        text=True,
        check=True,
    ).stdout
    match = re.search(r'sparkle:edSignature="([^"]+)"\s+length="(\d+)"', output)
    if not match:
        sys.exit(f"Unexpected sign_update output: {output!r}")

    with open(args.info_plist, "rb") as handle:
        info = plistlib.load(handle)
    record = {
        "arch": args.arch,
        "asset_name": Path(args.dmg).name,
        "ed_signature": match.group(1),
        "length": int(match.group(2)),
        "version": info["CFBundleVersion"],
        "short_version": info.get("CFBundleShortVersionString", info["CFBundleVersion"]),
        "minimum_system_version": info.get("LSMinimumSystemVersion"),
    }
    Path(args.out).write_text(json.dumps(record, indent=2) + "\n")
    print(f"Signed {record['asset_name']} for Sparkle ({record['version']})")


def feed(args: argparse.Namespace) -> None:
    ET.register_namespace("sparkle", SPARKLE_NS)
    notes_html = Path(args.notes_html).read_text() if args.notes_html else ""
    release_url = f"https://github.com/{args.repo}/releases/tag/{args.tag}"
    pub_date = email.utils.formatdate(usegmt=True)
    out_dir = Path(args.out_dir)
    out_dir.mkdir(parents=True, exist_ok=True)

    for record_path in args.records:
        record = json.loads(Path(record_path).read_text())
        rss = ET.Element("rss", {"version": "2.0"})
        channel = ET.SubElement(rss, "channel")
        ET.SubElement(channel, "title").text = "Termy"
        ET.SubElement(channel, "link").text = f"https://github.com/{args.repo}/releases"
        ET.SubElement(channel, "language").text = "en"

        item = ET.SubElement(channel, "item")
        ET.SubElement(item, "title").text = f"Termy {record['short_version']}"
        ET.SubElement(item, "pubDate").text = pub_date
        ET.SubElement(item, f"{{{SPARKLE_NS}}}version").text = record["version"]
        ET.SubElement(item, f"{{{SPARKLE_NS}}}shortVersionString").text = record[
            "short_version"
        ]
        if record.get("minimum_system_version"):
            ET.SubElement(item, f"{{{SPARKLE_NS}}}minimumSystemVersion").text = record[
                "minimum_system_version"
            ]
        ET.SubElement(item, f"{{{SPARKLE_NS}}}fullReleaseNotesLink").text = release_url
        if notes_html.strip():
            ET.SubElement(item, "description").text = notes_html
        ET.SubElement(
            item,
            "enclosure",
            {
                "url": f"https://github.com/{args.repo}/releases/download/{args.tag}/{record['asset_name']}",
                "length": str(record["length"]),
                "type": "application/octet-stream",
                f"{{{SPARKLE_NS}}}edSignature": record["ed_signature"],
            },
        )

        ET.indent(rss)
        path = out_dir / f"appcast-{record['arch']}.xml"
        ET.ElementTree(rss).write(path, encoding="utf-8", xml_declaration=True)
        print(f"Wrote {path}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)

    sign_parser = commands.add_parser("sign", help="EdDSA-sign a DMG for Sparkle")
    sign_parser.add_argument("--sign-update", required=True, help="Path to Sparkle's sign_update")
    sign_parser.add_argument("--dmg", required=True)
    sign_parser.add_argument("--info-plist", required=True, help="Info.plist of the app in the DMG")
    sign_parser.add_argument("--arch", required=True)
    sign_parser.add_argument("--out", required=True, help="JSON record to write")
    sign_parser.set_defaults(run=sign)

    feed_parser = commands.add_parser("feed", help="Write appcast-<arch>.xml files")
    feed_parser.add_argument("--repo", required=True, help="owner/name")
    feed_parser.add_argument("--tag", required=True)
    feed_parser.add_argument("--notes-html", help="Release notes rendered as HTML")
    feed_parser.add_argument("--out-dir", required=True)
    feed_parser.add_argument("records", nargs="+", help="JSON records from `sign`")
    feed_parser.set_defaults(run=feed)

    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
