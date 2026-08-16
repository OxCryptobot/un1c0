"""Deterministic source-risk fingerprinting.

The score is normalized to ``0.0..1.0``. It is a coarse signal for suspicious
uniformity, not a proof that code is malicious; callers should combine it with
parser validation, policy, and resource limits.
"""

import math
from typing import Tuple

ENTROPY_REJECTION_THRESHOLD = 0.92


def entropy_fingerprint(source: str) -> Tuple[float, bool]:
    """Return ``(normalized_entropy, reject)`` for a source string.

    The previous implementation divided Shannon entropy by the entropy of the
    *observed* alphabet while treating a one-symbol alphabet as ``1.0``. That
    made the documented gate mathematically unreachable at ``> 1.05`` and
    mis-scored repetitive input. This implementation returns a stable score in
    ``0.0..1.0`` and rejects only when the normalized score exceeds the shared
    0.92 policy threshold.
    """
    if not source.strip():
        return 0.0, False

    frequencies: dict[str, int] = {}
    for character in source:
        frequencies[character] = frequencies.get(character, 0) + 1

    length = len(source)
    distinct = len(frequencies)
    if length <= 1 or distinct <= 1:
        return 0.0, False

    actual = -sum(
        (count / length) * math.log2(count / length)
        for count in frequencies.values()
        if count
    )
    maximum = math.log2(distinct)
    score = min(1.0, max(0.0, actual / maximum)) if maximum else 0.0
    return score, score > ENTROPY_REJECTION_THRESHOLD


# Hard gate — used by all ingress paths.
def reject_if_obfuscated(source: str, lang: str) -> None:
    score, rejected = entropy_fingerprint(source)
    if rejected:
        raise ValueError(
            f"UN1C⓪ REJECT: {lang} source entropy {score:.3f} > "
            f"{ENTROPY_REJECTION_THRESHOLD:.2f} limit → OBFUSCATION DETECTED\n"
            "The input must also pass parser, policy, and resource validation."
        )
