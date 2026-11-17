"""A small module."""

import os
from dataclasses import dataclass

LIMIT = 10


@dataclass
class Point:
    x: float
    y: float = 0.0

    def norm(self) -> float:
        """Distance from the origin."""
        return (self.x**2 + self.y**2) ** 0.5


def main(args: list[str]) -> int:
    points = [Point(float(i)) for i in range(LIMIT)]
    for point in points:
        if point.norm() > 5:
            print(f"big {point.x}")
        elif point.x is None:
            raise ValueError("no x")
    # Done.
    return len(os.environ) and 0


if __name__ == "__main__":
    main([])
