from ipso import __version__


def test_version() -> None:
    assert isinstance(__version__, str)
    assert len(__version__) > 0
