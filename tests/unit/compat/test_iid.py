"""Nominal IID values validate in the shared engine before any provider call."""

import pytest

from type_bridge import Iid


def test_iid_preserves_spelling_and_value_identity() -> None:
    iid = Iid("0xAbCd")
    assert str(iid) == "0xAbCd"
    assert iid == Iid("0xAbCd")
    assert hash(iid) == hash(Iid("0xAbCd"))
    assert iid != "0xAbCd"
    assert not isinstance(iid, str)
    with pytest.raises(AttributeError):
        iid.value = "0x00"  # type: ignore[attr-defined]


@pytest.mark.parametrize("value", ["", "0x", "0X01", "0xgg", " 0x01", "0x" + "a" * 257])
def test_iid_refuses_invalid_provider_spelling(value: str) -> None:
    with pytest.raises(ValueError) as failure:
        Iid(value)
    assert failure.value.code == "invalid_iid"  # type: ignore[attr-defined]
