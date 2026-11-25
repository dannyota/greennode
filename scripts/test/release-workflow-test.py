import re
import unittest
from pathlib import Path


WORKFLOW = Path(__file__).resolve().parents[2] / ".github/workflows/release.yml"


def strip_comments(text):
    result = []
    for line in text.splitlines():
        if len(line) - len(line.lstrip()) > 6:
            result.append(line)
            continue
        quote = None
        escaped = False
        for index, char in enumerate(line):
            if escaped:
                escaped = False
            elif char == "\\" and quote == '"':
                escaped = True
            elif char in "\"'":
                if quote is None:
                    quote = char
                elif quote == char:
                    quote = None
            elif char == "#" and quote is None and (index == 0 or line[index - 1].isspace()):
                line = line[:index].rstrip()
                break
        result.append(line)
    return "\n".join(result) + "\n"


def validate(text):
    text = strip_comments(text)
    trigger = re.search(r"(?ms)^on:\n(.*?)(?=^[^ \n])", text)
    assert trigger and trigger.group(1).strip() == 'push:\n    tags: ["v*"]', "trigger"
    assert re.search(r"(?m)^permissions: \{\}$", text), "default permissions"
    jobs = re.split(r"(?m)^  ([a-z][a-z-]*):\n", text.split("\njobs:\n", 1)[1])
    sections = dict(zip(jobs[1::2], jobs[2::2]))
    assert "release" in sections, "release job"
    assert next(iter(sections)) == "verify", "verify must be first"
    assert 'concurrency:\n  group: release\n  cancel-in-progress: false\n' in text, "release concurrency"
    verify = sections["verify"]
    for required in ('fetch-depth: 0', 'persist-credentials: false', 'git merge-base --is-ancestor "$commit" origin/master', 'refs/tags/$TAG^{commit}', '$EVENT_SHA^{commit}', 'commit=$commit'):
        assert required in verify, f"verify {required}"
    for name, section in sections.items():
        if name != "verify":
            assert re.search(r"(?m)^    needs: (?:\[.*\bverify\b.*\]|verify)$", section), f"{name} needs verify"
            assert 'ref: ${{ needs.verify.outputs.commit }}' in section, f"{name} checkout ref"
            assert 'test "$(git rev-parse HEAD)" = "$COMMIT"' in section, f"{name} checkout validation"
        permissions = re.findall(r"(?m)^    permissions:\n((?:      [^\n]+\n)+)", section)
        assert len(permissions) == 1, f"{name} permissions"
        writes = re.findall(r"(?m)^      ([\w-]+): write$", permissions[0])
        assert writes == (["contents"] if name == "release" else []), f"{name} write permissions"
        assert not re.search(r"(?m)^    environment:", section), f"{name} environment"
        steps = re.split(r"(?m)^      - ", section)[1:]
        for step in steps:
            if any(command in step for command in ('cargo ', '__command-inventory', 'scripts/release-notes.sh', 'scripts/check-crate-release.py')):
                assert 'GH_TOKEN:' not in step and 'github.token' not in step, f"{name} token on tagged code"
    assert sections['inventory'].index('name: Find preceding release') < sections['inventory'].index('name: Check shipped commands'), "inventory token before tagged code"
    assert sections['notes'].index('name: Fetch release note data') < sections['notes'].index('scripts/release-notes.sh'), "notes token before tagged code"
    assert 'scripts/release-notes.sh' not in sections['release'], "release executes tagged script"
    assert 'name: release-notes' in sections['notes'] and 'artifacts/release-notes/notes.md' in sections['release'], "notes artifact"
    assert 'needs.inventory.outputs.previous' in sections['release'], "previous release identity"
    assert 'test "$previous" = "$EXPECTED_PREVIOUS"' in sections['release'], "previous release recheck"
    assert 'git/ref/tags/' in sections['release'] and 'test "$sha" = "$COMMIT"' in sections['release'], "live tag recheck"
    publish = sections['release'].split('      - name: Publish draft\n', 1)[1]
    assert publish.index('test "$previous" = "$EXPECTED_PREVIOUS"') < publish.index('git/ref/tags/') < publish.index('test "$sha" = "$COMMIT"') < publish.index('gh release edit'), "publish checks order"
    assert 'if [ -e .cargo/config ] || [ -L .cargo/config ] || [ -e .cargo/config.toml ] || [ -L .cargo/config.toml ]; then\n            exit 1\n          fi' in sections['receipt'], "cargo config guard"
    assert '"target/$TARGET/release/greennode$EXT" __command-inventory > binary/inventory.json' in sections['build'], "matrix inventory export"
    assert re.search(r'(?m)^          path: binary/$', sections['build']), "matrix inventory artifact"
    for platform in ('darwin-arm64', 'linux-amd64', 'linux-arm64', 'windows-amd64'):
        assert f'platform: {platform}' in sections['build'], f"missing {platform} build"
        assert platform in sections['inventory'], f"missing {platform} inventory"
    assert 'cargo build' not in sections['inventory'], "no separate inventory build"
    assert 'cmp "$candidate" "$other"' in sections['inventory'], "identical inventories"
    assert 'shipped "$candidate" "$committed" "$RUNNER_TEMP/previous.json"' in sections['inventory'], "previous inventory gate"
    assert 'shipped "$candidate" "$committed" --first' in sections['inventory'], "first inventory gate"
    assert sections['inventory'].index('cmp "$candidate" "$other"') < sections['inventory'].index('shipped "$candidate"'), "verified candidate inventory"


class WorkflowTest(unittest.TestCase):
    def test_release(self):
        validate(WORKFLOW.read_text())

    def test_commented_permission_and_trigger_regressions(self):
        text = WORKFLOW.read_text()
        cases = (
            ('    tags: ["v*"]\n\n', '    tags: ["v*"]\n\n  workflow_dispatch:\n\n'),
            ('      contents: read\n', '      contents: write # elevated\n'),
            ('      contents: write\n', '      contents: write\n      id-token: write # elevated\n'),
        )
        for old, new in cases:
            with self.subTest(old=old):
                self.assertIn(old, text)
                with self.assertRaises(AssertionError):
                    validate(text.replace(old, new, 1))

    def test_legitimate_inline_comments(self):
        text = WORKFLOW.read_text()
        text = text.replace('    tags: ["v*"]\n', '    tags: ["v*"] # release tags\n', 1)
        text = text.replace('      contents: read\n', '      contents: read # read only\n', 1)
        text = text.replace('      contents: write\n', '      contents: write # release only\n', 1)
        validate(text)

    def test_hostile_mutations(self):
        text = WORKFLOW.read_text()
        cases = (
            ('tags: ["v*"]', 'branches: [master]'),
            ('permissions: {}', 'permissions: read-all'),
            ('      contents: write', '      contents: read'),
            ('      contents: read', '      id-token: write'),
            ('    permissions:\n', '    environment: crates-io\n    permissions:\n'),
            ('  group: release\n', '  group: other\n'),
            ('git merge-base --is-ancestor', 'true #'),
            ('git merge-base --is-ancestor "$commit" origin/master', 'git merge-base --is-ancestor "$commit" origin/other'),
            ('commit=$commit', 'other=$commit'),
            ('refs/tags/$TAG^{commit}', 'refs/heads/master^{commit}'),
            ('$EVENT_SHA^{commit}', '$TAG^{commit}'),
            ('    needs: verify\n', '    needs: checks\n'),
            ('ref: ${{ needs.verify.outputs.commit }}', 'ref: ${{ github.ref }}'),
            ('test "$(git rev-parse HEAD)" = "$COMMIT"', 'true'),
            ('      - name: Check shipped commands\n        env:\n', '      - name: Check shipped commands\n        env:\n          GH_TOKEN: ${{ github.token }}\n'),
            ('name: release-notes\n', 'name: other-notes\n'),
            ('artifacts/release-notes/notes.md', 'notes.md'),
            ('needs.inventory.outputs.previous', 'github.ref'),
            ('test "$previous" = "$EXPECTED_PREVIOUS"', 'true'),
            ('git/ref/tags/', 'git/refs/heads/'),
            ('test "$sha" = "$COMMIT"', 'true'),
            ('if [ -e .cargo/config ] || [ -L .cargo/config ] || [ -e .cargo/config.toml ] || [ -L .cargo/config.toml ]; then', 'if false; then'),
            ('__command-inventory > binary/inventory.json', '__command-inventory > /dev/null'),
            ('path: binary/', 'path: binary/greennode'),
            ('cmp "$candidate" "$other"', 'true'),
            ('shipped "$candidate"', 'shipped /dev/null'),
        )
        for old, new in cases:
            with self.subTest(old=old):
                self.assertIn(old, text)
                with self.assertRaises(AssertionError):
                    validate(text.replace(old, new, 1))


if __name__ == "__main__":
    unittest.main()
