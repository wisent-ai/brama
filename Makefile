# The commands this repository is built and tested with.
#
# They exist as a checked-in file rather than as lines somebody types, so the
# build a person runs here, the one a Stado recipe runs on every commit and the
# one a release candidate is verified with are the same three words.

.PHONY: check build release test

# Does the working copy compile? The cheap question, and the one to ask after
# an edit: a build is rationed, a check is not.
check:
	cargo check --locked

# Debug build of the whole workspace, locked to the committed dependency graph.
build:
	cargo build --locked

# What a delivery installs.
release:
	cargo build --locked --release

# The repository's own suites.
test:
	cargo test --locked
