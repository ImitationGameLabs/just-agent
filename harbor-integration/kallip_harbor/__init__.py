"""Harbor benchmarking adapter for kallip."""

import importlib

__all__ = ["KallipAdapter"]


def __getattr__(name: str):
    # Lazy: importing the adapter pulls in harbor, which is only
    # installed inside benchmarking venvs. Harbor-free consumers
    # (unit tests, path helpers) import kallip_harbor.tagma directly.
    if name == "KallipAdapter":
        return importlib.import_module("kallip_harbor.adapter").KallipAdapter
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
