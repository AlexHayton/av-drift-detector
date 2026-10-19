#!/usr/bin/env python3
"""Keep every package in this repository on one version number, and cut releases.

    scripts/version.py show                  print the current version
    scripts/version.py check [--tag vX.Y.Z]  check every manifest agrees (and matches a tag)
    scripts/version.py bump PART|VERSION     bump everything, date the changelog, commit and tag
    scripts/version.py notes VERSION         print a version's CHANGELOG.md section

PART is major, minor or patch. VERSION is X.Y.Z, or a pre-release X.Y.Z-alpha.N,
X.Y.Z-beta.N or X.Y.Z-rc.N: the forms that crates.io, npm and PyPI all accept and
order the same way. PyPI packages get the PEP 440 spelling (X.Y.ZaN, bN, rcN).

Manifests are the files git knows about (committed, or new and not ignored):
- every Cargo.toml with a [package] version, unless it has publish = false;
- every package.json with a version, and the package-lock.json beside it;
- every pyproject.toml with a static [project] version.
Cargo.lock, version requirements on this repository's own packages, and
`<crate> = "X.Y"` dependency snippets in Markdown files are updated with them.
"""

from __future__ import annotations

import argparse
import datetime
import json
import os
import re
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

CHANGELOG = "CHANGELOG.md"
RELEASE_BRANCH = "main"
TAG_PREFIX = "v"

PRE_KINDS = ("alpha", "beta", "rc")
PEP440_PRE = {"alpha": "a", "beta": "b", "rc": "rc"}
SEMVER_RE = re.compile(
    r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-(alpha|beta|rc)\.(0|[1-9]\d*))?"
)
PEP440_RE = re.compile(r"(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:(a|b|rc)(0|[1-9]\d*))?")
NPM_DEP_SECTIONS = ("dependencies", "devDependencies", "optionalDependencies", "peerDependencies")


class Error(Exception):
    pass


@dataclass(frozen=True)
class Version:
    major: int
    minor: int
    patch: int
    pre: tuple[str, int] | None = None

    @classmethod
    def parse(cls, text: str) -> Version:
        text = text.strip()
        if text.startswith(TAG_PREFIX):
            text = text[len(TAG_PREFIX) :]
        if m := SEMVER_RE.fullmatch(text):
            pre = (m[4], int(m[5])) if m[4] else None
        elif m := PEP440_RE.fullmatch(text):
            kinds = {short: kind for kind, short in PEP440_PRE.items()}
            pre = (kinds[m[4]], int(m[5])) if m[4] else None
        else:
            raise Error(
                f"{text!r} is not a supported version: use X.Y.Z, or X.Y.Z-alpha.N, -beta.N or -rc.N"
            )
        return cls(int(m[1]), int(m[2]), int(m[3]), pre)

    def key(self) -> tuple[int, ...]:
        pre = (1, 0, 0) if self.pre is None else (0, PRE_KINDS.index(self.pre[0]), self.pre[1])
        return (self.major, self.minor, self.patch, *pre)

    def semver(self) -> str:
        base = f"{self.major}.{self.minor}.{self.patch}"
        return base if self.pre is None else f"{base}-{self.pre[0]}.{self.pre[1]}"

    def pep440(self) -> str:
        base = f"{self.major}.{self.minor}.{self.patch}"
        return base if self.pre is None else f"{base}{PEP440_PRE[self.pre[0]]}{self.pre[1]}"

    def tag(self) -> str:
        return TAG_PREFIX + self.semver()

    def bumped(self, part: str) -> Version:
        # npm's rules: bumping a pre-release gives the release it leads up to.
        if part == "major":
            if self.pre and self.minor == 0 and self.patch == 0:
                return Version(self.major, 0, 0)
            return Version(self.major + 1, 0, 0)
        if part == "minor":
            if self.pre and self.patch == 0:
                return Version(self.major, self.minor, 0)
            return Version(self.major, self.minor + 1, 0)
        if part == "patch":
            if self.pre:
                return Version(self.major, self.minor, self.patch)
            return Version(self.major, self.minor, self.patch + 1)
        raise Error(f"unknown version part {part!r}")

    def __str__(self) -> str:
        return self.semver()


@dataclass
class Manifest:
    path: str
    kind: str  # "cargo", "npm" or "python"
    name: str
    version: Version


def run(*args: str, check: bool = True) -> str:
    result = subprocess.run(args, capture_output=True, text=True)
    if check and result.returncode != 0:
        raise Error(f"`{' '.join(args)}` failed:\n{result.stderr.strip()}")
    return result.stdout.rstrip()


def read(path: str) -> str:
    return Path(path).read_text(encoding="utf-8")


def write(path: str, text: str) -> None:
    Path(path).write_text(text, encoding="utf-8")


def git_files() -> list[str]:
    out = run("git", "ls-files", "--cached", "--others", "--exclude-standard", "-z")
    return sorted({f for f in out.split("\0") if f and Path(f).is_file()})


def toml_section(text: str, header: str) -> tuple[int, int] | None:
    """Byte range of the body of a [header] table."""
    m = re.search(rf"(?m)^\[{re.escape(header)}\][ \t]*(?:#.*)?$", text)
    if not m:
        return None
    following = re.search(r"(?m)^\[", text[m.end() :])
    return m.end(), m.end() + following.start() if following else len(text)


def toml_string(body: str, key: str) -> re.Match[str] | None:
    return re.search(rf'(?m)^{key}[ \t]*=[ \t]*"([^"]*)"', body)


def set_toml_string(text: str, header: str, key: str, value: str) -> str:
    start, end = toml_section(text, header)
    m = toml_string(text[start:end], key)
    a, b = start + m.start(1), start + m.end(1)
    return text[:a] + value + text[b:]


def read_manifest(path: str, text: str) -> Manifest | None:
    name = Path(path).name
    if name == "Cargo.toml":
        span = toml_section(text, "package")
        if not span:
            return None
        body = text[span[0] : span[1]]
        if re.search(r"(?m)^publish[ \t]*=[ \t]*false", body):
            return None
        version, crate = toml_string(body, "version"), toml_string(body, "name")
        if version and crate:
            return Manifest(path, "cargo", crate[1], Version.parse(version[1]))
    elif name == "package.json":
        data = json.loads(text)
        if "version" in data:
            return Manifest(path, "npm", data.get("name", path), Version.parse(data["version"]))
    elif name == "pyproject.toml":
        span = toml_section(text, "project")
        if not span:
            return None
        body = text[span[0] : span[1]]
        version, project = toml_string(body, "version"), toml_string(body, "name")
        if version and project:
            return Manifest(path, "python", project[1], Version.parse(version[1]))
    return None


def discover(files: list[str]) -> list[Manifest]:
    manifests = []
    for path in files:
        if Path(path).name in ("Cargo.toml", "package.json", "pyproject.toml"):
            try:
                found = read_manifest(path, read(path))
            except Error as e:
                raise Error(f"{path}: {e}") from None
            if found:
                manifests.append(found)
    if not manifests:
        raise Error("no versioned manifests found (run this inside the repository)")
    return manifests


def current_version(manifests: list[Manifest]) -> Version:
    versions = {m.version for m in manifests}
    if len(versions) > 1:
        rows = "\n".join(f"  {m.path}: {m.version}" for m in manifests)
        raise Error(f"manifests disagree on the version:\n{rows}")
    return versions.pop()


def indent_of(text: str) -> int:
    m = re.search(r"\n( +)\S", text)
    return len(m[1]) if m else 2


def dump_json(data: object, like: str) -> str:
    return json.dumps(data, indent=indent_of(like), ensure_ascii=False) + "\n"


def bump_npm_ref(spec: str, old: Version, new: Version) -> str:
    m = re.fullmatch(r"([\^~=]?)(.+)", spec)
    if m and m[2] == old.semver():
        return m[1] + new.semver()
    return spec


def plan_edits(files: list[str], manifests: list[Manifest], new: Version) -> dict[str, str]:
    """New contents of every file the bump changes (manifests, lockfiles, docs)."""
    old = current_version(manifests)
    crates = [m.name for m in manifests if m.kind == "cargo"]
    npm_names = {m.name for m in manifests if m.kind == "npm"}
    edits: dict[str, str] = {}

    def text(path: str) -> str:
        return edits[path] if path in edits else read(path)

    for m in manifests:
        content = text(m.path)
        if m.kind == "cargo":
            edits[m.path] = set_toml_string(content, "package", "version", new.semver())
        elif m.kind == "python":
            edits[m.path] = set_toml_string(content, "project", "version", new.pep440())
        elif m.kind == "npm":
            data = json.loads(content)
            data["version"] = new.semver()
            for section in NPM_DEP_SECTIONS:
                for dep, spec in data.get(section, {}).items():
                    if dep in npm_names:
                        data[section][dep] = bump_npm_ref(spec, old, new)
            edits[m.path] = dump_json(data, content)
            lock = str(Path(m.path).with_name("package-lock.json"))
            if lock in files:
                lock_text = text(lock)
                data = json.loads(lock_text)
                data["version"] = new.semver()
                if "" in data.get("packages", {}):
                    data["packages"][""]["version"] = new.semver()
                edits[lock] = dump_json(data, lock_text)

    def requirement(m: re.Match[str]) -> str:
        return m[1] + new.semver() + m[3] if "path" in m[0] else m[0]

    def snippet(m: re.Match[str]) -> str:
        parts = [new.major, new.minor, new.patch][: m[2].count(".") + 1]
        return m[1] + ".".join(map(str, parts)) + m[3]

    for path in files:
        name = Path(path).name
        # Pre-releases leave the docs' dependency snippets alone.
        is_doc = name.endswith(".md") and path != CHANGELOG and new.pre is None
        if name not in ("Cargo.toml", "Cargo.lock") and not is_doc:
            continue
        content = updated = text(path)
        for crate in map(re.escape, crates):
            if name == "Cargo.toml":
                # Requirements on our own crates: `crate = { path = "..", version = "0.1.0" }`.
                pattern = rf'(?m)^([ \t]*{crate}[ \t]*=[ \t]*\{{.*\bversion[ \t]*=[ \t]*"[=^~]?)([^"]*)(".*)$'
                updated = re.sub(pattern, requirement, updated)
            elif name == "Cargo.lock":
                pattern = rf'(?m)^(name = "{crate}"\nversion = ")([^"]*)(")'
                updated = re.sub(pattern, lambda m: m[1] + new.semver() + m[3], updated)
            else:
                # `crate = "0.1"` or `crate = { version = "0.1", ... }`, keeping however
                # many components the snippet was written with.
                pattern = rf'(?m)^([ \t]*{crate}[ \t]*=[ \t]*(?:\{{[ \t]*version[ \t]*=[ \t]*)?")(\d+(?:\.\d+){{0,2}})(")'
                updated = re.sub(pattern, snippet, updated)
        if updated != content:
            edits[path] = updated

    return {path: content for path, content in edits.items() if content != read(path)}


def repository_url(files: list[str]) -> str | None:
    if "Cargo.toml" not in files:
        return None
    text = read("Cargo.toml")
    span = toml_section(text, "package")
    m = span and toml_string(text[span[0] : span[1]], "repository")
    return re.sub(r"(\.git)?/?$", "", m[1]) if m else None


def section_span(text: str, title: str) -> tuple[int, int] | None:
    """Range of a `## [title]` section's body, up to the next section or the link list."""
    m = re.search(rf"(?m)^## \[{re.escape(title)}\][^\n]*\n", text)
    if not m:
        return None
    following = re.search(r"(?m)^## \[|^\[[^\]]+\]:[ \t]", text[m.end() :])
    return m.end(), m.end() + following.start() if following else len(text)


def date_changelog(text: str, new: Version, previous: str | None, repo: str | None, date: str) -> str:
    span = section_span(text, "Unreleased")
    if span is None:
        raise Error(f"{CHANGELOG} has no '## [Unreleased]' section")
    if section_span(text, new.semver()):
        raise Error(f"{CHANGELOG} already has a section for {new}")
    notes = text[span[0] : span[1]].strip()
    if not notes:
        raise Error(f"the Unreleased section of {CHANGELOG} is empty: describe the changes first")
    heading = re.search(r"(?m)^## \[Unreleased\]", text).start()
    rest = text[span[1] :].lstrip("\n")
    text = f"{text[:heading]}## [Unreleased]\n\n## [{new}] - {date}\n\n{notes}\n\n{rest}"
    if repo:
        unreleased = f"[Unreleased]: {repo}/compare/{new.tag()}...HEAD"
        if previous:
            this = f"[{new}]: {repo}/compare/{previous}...{new.tag()}"
        else:
            this = f"[{new}]: {repo}/releases/tag/{new.tag()}"
        if re.search(r"(?m)^\[Unreleased\]:.*$", text):
            text = re.sub(r"(?m)^\[Unreleased\]:.*$", lambda _: f"{unreleased}\n{this}", text, count=1)
        else:
            text = text.rstrip("\n") + f"\n\n{unreleased}\n{this}\n"
    return text


def release_notes(text: str, version: str) -> str:
    span = section_span(text, Version.parse(version).semver())
    if span is None:
        raise Error(f"{CHANGELOG} has no section for {version}")
    return text[span[0] : span[1]].strip()


def tags() -> list[Version]:
    found = []
    for tag in run("git", "tag", "--list", f"{TAG_PREFIX}*").splitlines():
        try:
            found.append(Version.parse(tag))
        except Error:
            pass
    return sorted(found, key=Version.key)


def git_problems() -> list[str]:
    problems = []
    branch = run("git", "symbolic-ref", "--quiet", "--short", "HEAD", check=False)
    if branch != RELEASE_BRANCH:
        problems.append(f"releases are made from {RELEASE_BRANCH}, but this is {branch or 'a detached HEAD'}")
    changed = [line[3:] for line in run("git", "status", "--porcelain", "--untracked-files=no").splitlines()]
    changed = [path for path in changed if path != CHANGELOG]
    if changed:
        problems.append("commit or stash these changes first: " + ", ".join(changed))
    return problems


def cargo_lock_ok() -> bool:
    if not Path("Cargo.lock").exists() or not shutil.which("cargo"):
        return True
    result = subprocess.run(
        ["cargo", "metadata", "--locked", "--offline", "--format-version", "1"],
        capture_output=True,
        text=True,
    )
    return result.returncode == 0


def bump(target: str, dry_run: bool, push: bool, date: str | None) -> None:
    files = git_files()
    manifests = discover(files)
    old = current_version(manifests)
    new = old.bumped(target) if target in ("major", "minor", "patch") else Version.parse(target)
    existing = tags()

    if new.key() < old.key():
        raise Error(f"{new} is older than the current version {old}")
    if any(t == new for t in existing):
        raise Error(f"{new.tag()} already exists")
    problems = git_problems()
    if problems and not dry_run:
        raise Error("\n".join(problems))

    edits = plan_edits(files, manifests, new)
    changelog = read(CHANGELOG)
    previous = max((t for t in existing if t.key() < new.key()), key=Version.key, default=None)
    edits[CHANGELOG] = date_changelog(
        changelog,
        new,
        previous.tag() if previous else None,
        repository_url(files),
        date or datetime.date.today().isoformat(),
    )

    print(f"{old} -> {new}" if new != old else f"releasing {new}")
    for m in manifests:
        shown = new.pep440() if m.kind == "python" else new.semver()
        print(f"  {m.path:<40} {m.kind} {m.name} {shown}")
    for path in sorted(set(edits) - {m.path for m in manifests}):
        print(f"  {path}")
    if dry_run:
        for problem in problems:
            print(f"warning: {problem}")
        print("dry run: nothing written")
        return

    originals = {path: read(path) for path in edits}
    for path, content in edits.items():
        write(path, content)
    if not cargo_lock_ok():
        for path, content in originals.items():
            write(path, content)
        raise Error(
            "Cargo.lock doesn't match the manifests after the bump, so nothing was changed. "
            "Run `cargo update --workspace` and try again."
        )

    run("git", "add", "--", *sorted(edits))
    run("git", "commit", "--quiet", "-m", f"Release {new.tag()}")
    run("git", "tag", "--annotate", new.tag(), "-m", f"Release {new.tag()}")
    print(f"committed and tagged {new.tag()}")
    if push:
        run("git", "push", "--atomic", "origin", RELEASE_BRANCH, new.tag())
        print(f"pushed {RELEASE_BRANCH} and {new.tag()}: the Release workflow takes it from here")
    else:
        print(f"publish it with: git push --atomic origin {RELEASE_BRANCH} {new.tag()}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="scripts/version.py",
        description=__doc__.split("\n\n")[0],
        epilog="See docs/development.md#releasing.",
    )
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("show", help="print the current version")
    check = commands.add_parser("check", help="check that every manifest has the same version")
    check.add_argument("--tag", help="also require the version to match this tag")
    bump_cmd = commands.add_parser("bump", help="bump every package, date the changelog, commit and tag")
    bump_cmd.add_argument("target", metavar="major|minor|patch|VERSION")
    bump_cmd.add_argument("--dry-run", action="store_true", help="show what would change, write nothing")
    bump_cmd.add_argument("--push", action="store_true", help="push the release commit and tag to origin")
    bump_cmd.add_argument("--date", help=argparse.SUPPRESS)
    notes = commands.add_parser("notes", help="print the CHANGELOG.md section for a version")
    notes.add_argument("version")
    args = parser.parse_args(argv)

    try:
        os.chdir(run("git", "rev-parse", "--show-toplevel"))
        if args.command == "show":
            print(current_version(discover(git_files())))
        elif args.command == "check":
            manifests = discover(git_files())
            version = current_version(manifests)
            if args.tag and Version.parse(args.tag) != version:
                raise Error(f"tag {args.tag} doesn't match the version in the manifests ({version})")
            print(f"{version}: {len(manifests)} manifest(s) agree")
        elif args.command == "bump":
            bump(args.target, args.dry_run, args.push, args.date)
        elif args.command == "notes":
            print(release_notes(read(CHANGELOG), args.version))
    except Error as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
