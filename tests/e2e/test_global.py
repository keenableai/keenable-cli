"""T-41 — version, help tree, unknown subcommand."""

import re


def test_t41_version(kn):
    res = kn("--version", key=False)
    assert res.code == 0
    assert re.match(r"^keenable \d+\.\d+\.\d+", res.out)


def test_t41_help_lists_subcommands(kn):
    res = kn("--help", key=False)
    assert res.code == 0
    for sub in ("login", "logout", "configure-mcp", "reset", "config", "search", "fetch", "feedback"):
        assert sub in res.out, f"--help missing subcommand {sub}"


def test_t41_subcommand_help(kn):
    res = kn("search", "--help", key=False)
    assert res.code == 0
    for flag in ("--mode", "--site", "--acquired-after", "--published-before", "--pretty", "--api-key"):
        assert flag in res.out, f"search --help missing {flag}"


def test_t41_unknown_subcommand(kn):
    # 0.1.15 exited 0 here; current clap exits 2.
    res = kn("frobnicate", key=False)
    assert res.code == 2
    assert "unrecognized subcommand 'frobnicate'" in res.err
