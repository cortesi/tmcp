#!/usr/bin/env python3
"""
Sync weather_server.rs example to lib.rs and README.md

This script ensures that the weather server example in the canonical source
(examples/weather_server.rs) stays in sync with the module docstring and README.
"""

import re
from pathlib import Path

# Define paths relative to the project root
PROJECT_ROOT = Path(__file__).parent.parent
EXAMPLE_PATH = PROJECT_ROOT / "crates/tmcp/examples/weather_server.rs"
LIB_RS_PATH = PROJECT_ROOT / "crates/tmcp/src/lib.rs"
README_PATH = PROJECT_ROOT / "README.md"


def extract_example_code():
    """Extract the full weather_server.rs content."""
    with open(EXAMPLE_PATH, "r") as f:
        return f.read().strip()


def update_lib_rs(example_code):
    """Update the example in lib.rs docstring."""
    with open(LIB_RS_PATH, "r") as f:
        content = f.read()

    # Add //! prefix and proper indentation for doc comments
    doc_example = "\n".join(
        f"//! {line}" if line else "//!" for line in example_code.split("\n")
    )

    # Pattern to match the example block in lib.rs
    # Look for the rust,no_run code block
    pattern = r"(//! ```rust,no_run\n)(//!.*?\n)*?(//! ```)"

    # Replace with the new example
    replacement = f"\\1{doc_example}\n\\3"

    new_content = re.sub(pattern, replacement, content, flags=re.DOTALL)

    with open(LIB_RS_PATH, "w") as f:
        f.write(new_content)

    print(f"✓ Updated {LIB_RS_PATH}")


def update_readme(example_code):
    """Update the example in README.md."""
    with open(README_PATH, "r") as f:
        content = f.read()

    # Find all rust code blocks
    rust_blocks = list(re.finditer(r"```rust\n(.*?)\n```", content, flags=re.DOTALL))

    if not rust_blocks:
        print("✗ No rust code blocks found in README.md")
        return

    # Get the last rust code block
    last_block = rust_blocks[-1]

    # Replace the content of the last rust code block
    new_content = (
        content[: last_block.start(1)] + example_code + content[last_block.end(1) :]
    )

    with open(README_PATH, "w") as f:
        f.write(new_content)

    print(f"✓ Updated {README_PATH}")


def main():
    """Main sync function."""
    print("Syncing weather_server.rs example...")

    # Extract the canonical example
    example_code = extract_example_code()

    # Update both targets
    update_lib_rs(example_code)
    update_readme(example_code)

    print("\nDone! Weather server example is now synchronized.")


if __name__ == "__main__":
    main()
