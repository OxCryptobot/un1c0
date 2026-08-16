from ueg.entropy import entropy_fingerprint, reject_if_obfuscated


def test_empty_and_repeated_sources_are_not_rejected():
    ratio, rejected = entropy_fingerprint("")
    assert ratio == 0.0
    assert rejected is False

    ratio, rejected = entropy_fingerprint("aaaa\n")
    assert 0.0 < ratio < 1.0
    assert rejected is False


def test_reject_if_obfuscated_is_noop_for_normal_source():
    reject_if_obfuscated("def add(a, b):\n    return a + b\n", "python")


def test_reject_if_obfuscated_raises_for_uniform_source():
    source = "".join(chr(33 + index) for index in range(96))
    try:
        reject_if_obfuscated(source, "python")
    except ValueError as error:
        assert "OBFUSCATION DETECTED" in str(error)
    else:
        raise AssertionError("uniform high-entropy source must be rejected")
