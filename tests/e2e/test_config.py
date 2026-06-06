"""T-37..T-40 — config. Each test gets its own HOME (kn_fresh) so mutable
config state never bleeds into other tests."""


def test_set_get_unset_roundtrip(kn_fresh):
    res = kn_fresh("config", "set", "default_search_mode", "realtime", key=False)
    assert res.code == 0
    assert "default_search_mode = realtime" in res.err

    res = kn_fresh("config", "get", "default_search_mode", key=False)
    assert res.code == 0
    assert res.out.strip() == "realtime"

    res = kn_fresh("config", "unset", "default_search_mode", key=False)
    assert res.code == 0
    assert "default_search_mode unset" in res.err

    res = kn_fresh("config", key=False)
    assert "default_search_mode (not set" in res.err


def test_list_all(kn_fresh):
    res = kn_fresh("config", key=False)
    assert res.code == 0
    assert "default_search_mode" in res.err
    assert "forced_search_mode" in res.err
    assert "realtime, pro" in res.err


def test_invalid_value(kn_fresh):
    res = kn_fresh("config", "set", "default_search_mode", "bogus", key=False)
    assert res.code == 1
    assert 'Invalid value "bogus"' in res.err
    assert "Allowed: realtime, pro" in res.err


def test_forced_mode_overrides_flag(kn_fresh):
    assert kn_fresh("config", "set", "forced_search_mode", "realtime", key=False).code == 0
    data = kn_fresh("search", "x", "--mode", "pro").yaml()
    assert data["mode"] == "realtime"


def test_default_search_mode_applies_without_flag(kn_fresh):
    assert kn_fresh("config", "set", "default_search_mode", "realtime", key=False).code == 0
    data = kn_fresh("search", "x").yaml()
    assert data["mode"] == "realtime"
