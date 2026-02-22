```python
# Sample Python file for testing CodeConverter
# 한국어 주석 (Korean comment)


class Greeter:
    """A simple greeter class."""

    def __init__(self, name: str):
        self.name = name

    def greet(self) -> str:
        return f"Hello, {self.name}! 🚀"


def add(a: int, b: int) -> int:
    """Add two numbers."""
    return a + b


if __name__ == "__main__":
    g = Greeter("世界")
    print(g.greet())
    print(add(1, 2))
```
