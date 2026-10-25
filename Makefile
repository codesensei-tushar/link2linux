.PHONY: doctor ci ci-changed dev build test lint fmt rust-test rust-lint web-install web-test web-lint android android-test proto-lint

ANDROID := apps/android
GRADLE := cd $(ANDROID) && ./gradlew

doctor:
	@scripts/doctor.sh

# The checks a PR must pass (GitHub CI is manual-only).
ci:
	@scripts/ci.sh

ci-changed:
	@scripts/ci.sh --changed

web-install:
	pnpm install

dev: web-install
	pnpm --filter desktop tauri dev

build: web-install
	pnpm --filter desktop tauri build

rust-test:
	cargo test --workspace

rust-lint:
	cargo fmt --all --check
	cargo clippy --workspace --all-targets -- -D warnings

web-test: web-install
	pnpm --filter desktop test

web-lint: web-install
	pnpm --filter desktop lint
	pnpm --filter desktop typecheck

android:
	$(GRADLE) assembleDebug

android-test:
	$(GRADLE) testDebugUnitTest

proto-lint:
	buf lint

fmt:
	cargo fmt --all
	pnpm --filter desktop format

test: rust-test web-test android-test

lint: rust-lint web-lint
	$(GRADLE) lint
