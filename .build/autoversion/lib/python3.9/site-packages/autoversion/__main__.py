"""Allow `python -m autoversion`, so a caller that has the package on its path can
run the rule without installing a console script.

CI steps and repositories in other languages should not have to care whether an
entry point was installed.
"""

import sys

from autoversion.cli import main

if __name__ == "__main__":
    sys.exit(main())
