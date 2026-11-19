"""Print the license notices of every vendored crate.

usage: third-party-licenses.py VENDOR_DIR SPDX_TEXT_DIR

VENDOR_DIR is crane's vendored dependencies (<source>/<name>-<version>/).
Each crate's license and notice files are printed as they are, from the
crate root, a REUSE `LICENSES/` directory, and `license-file`. A crate with
no license file gets its authors and the SPDX text of each license its
expression names, and the script fails when such a text is missing, so a
copy never lacks a license.
"""
import re
import sys
import tomllib
from pathlib import Path

LICENSE = re.compile(r"^(LICEN[CS]E|COPYING|UNLICENSE)", re.IGNORECASE)
NOTICE = re.compile(r"^NOTICE", re.IGNORECASE)


def license_ids(expression):
    """The license and exception IDs an SPDX expression names, with the old
    `/` separator read as OR."""
    words = re.split(r"[\s/()]+", expression)
    return [word for word in words if word and word not in ("OR", "AND", "WITH")]


def spdx_text(spdx, license_id):
    """The SPDX text for `license_id`; `X+` is `X-or-later` or else `X`."""
    names = [license_id]
    if license_id.endswith("+"):
        names = [license_id[:-1] + "-or-later", license_id[:-1]]
    for name in names:
        if (spdx / f"{name}.txt").is_file():
            return spdx / f"{name}.txt"
    return None


def crate_files(crate, package):
    """The crate's license files and its notice files, by relative path."""
    candidates = [path for path in crate.iterdir() if path.is_file()]
    if (crate / "LICENSES").is_dir():
        candidates += [path for path in (crate / "LICENSES").iterdir() if path.is_file()]
    licenses = {path for path in candidates if LICENSE.match(path.name) or path.parent.name == "LICENSES"}
    if declared := package.get("license-file"):
        licenses.add(crate / declared)
    notices = {path for path in candidates if NOTICE.match(path.name)}
    order = lambda path: str(path.relative_to(crate))
    return sorted(licenses, key=order), sorted(notices - licenses, key=order)


def notices(vendor, spdx, out):
    """Writes every crate's notices to `out` and returns what is missing."""
    missing = []
    for crate in sorted(vendor.glob("*/*/"), key=lambda path: path.name):
        package = tomllib.loads((crate / "Cargo.toml").read_text())["package"]
        expression = package.get("license", "")
        print(f"== {crate.name}, license {expression or '(none)'}", file=out)
        licenses, notice_files = crate_files(crate, package)
        for path in licenses + notice_files:
            print(f"-- {path.relative_to(crate)}", file=out)
            print(path.read_text(errors="replace"), file=out)
        if not licenses:
            if authors := package.get("authors"):
                print("Authors: " + ", ".join(authors), file=out)
            ids = license_ids(expression)
            if not ids:
                missing.append(f"{crate.name}: no license file and no license expression")
            for license_id in ids:
                text = spdx_text(spdx, license_id)
                if text is None:
                    missing.append(f"{crate.name}: no license file and no SPDX text for {license_id}")
                    continue
                print(f"-- SPDX {license_id} (the crate ships no license file)", file=out)
                print(text.read_text(errors="replace"), file=out)
        print(file=out)
    return missing


if __name__ == "__main__":
    missing = notices(Path(sys.argv[1]), Path(sys.argv[2]), sys.stdout)
    if missing:
        sys.exit("\n".join(missing))
