"""Git history walker — populates the Layer 2 history overlay.

See docs/architecture-layered-graphs.md for the overall design.
"""

from savants.history.walker import (
    CommitInfo,
    GitHistoryWalker,
    HistoryWalkResult,
)

__all__ = ["CommitInfo", "GitHistoryWalker", "HistoryWalkResult"]
