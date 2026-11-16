"""Native 3.0 IID parsing, rejection and immutable identity operations."""

from type_bridge_core import Iid


def test_parse(benchmark):
    result = benchmark(Iid, "0x1e00000000000000000001")
    assert str(result) == "0x1e00000000000000000001"


def test_reject_invalid(benchmark):
    def reject():
        try:
            Iid("not-an-iid")
        except ValueError:
            return
        raise AssertionError("invalid IID was admitted")

    benchmark(reject)


def test_hash(benchmark):
    iid = Iid("0x1e00000000000000000001")
    assert benchmark(hash, iid) == hash(iid)
