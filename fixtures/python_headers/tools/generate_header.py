"""Generate one header; all input/output directories are prepared by the caller."""

import argparse
from pathlib import Path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("template", type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()

    template = args.template.read_text(encoding="utf-8")
    args.output.write_text(template.replace("@ANSWER@", "42"), encoding="utf-8")


if __name__ == "__main__":
    main()
