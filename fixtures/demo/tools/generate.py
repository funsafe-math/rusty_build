import pathlib
import sys
pathlib.Path(sys.argv[1]).write_text("#define BONUS 5\n", encoding="utf-8")
