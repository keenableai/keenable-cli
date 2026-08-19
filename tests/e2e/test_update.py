"""`keenable update` — self-update guard rails.

The real download path would replace the binary under test, so the suite
only exercises the deterministic no-receipt path: AXOUPDATER_CONFIG_PATH
points at an empty dir, so the CLI must refuse and hint at the installer.
"""


def test_update_without_receipt_fails_with_hint(kn_fresh, tmp_path, monkeypatch):
    monkeypatch.setenv("AXOUPDATER_CONFIG_PATH", str(tmp_path / "no-receipt"))
    res = kn_fresh("update", key=False)
    assert res.code == 1
    err = " ".join(res.err.split())  # undo the terminal word-wrap
    assert "cannot self-update" in err
    assert "Reinstall with" in err
