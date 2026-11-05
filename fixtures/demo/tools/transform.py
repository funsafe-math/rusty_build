import pathlib
import sys
pathlib.Path(sys.argv[2]).write_text(pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").upper(), encoding="utf-8")
