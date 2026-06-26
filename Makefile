.PHONY: bench bench-no-jit bench-diff
bench:
	cargo bench -p rune_bench 2>&1 | tee crates/rune_bench/results/$$(date +%Y%m%d_%H%M).txt

bench-no-jit:
	cargo bench -p rune_bench --no-default-features 2>&1 | tee crates/rune_bench/results/$$(date +%Y%m%d_%H%M)_nojit.txt

bench-diff:
	@cargo bench -p rune_bench 2>&1 | tee crates/rune_bench/results/$$(date +%Y%m%d_%H%M).txt
	@echo ""
	@echo "=== Latest result ==="
	@ls -t crates/rune_bench/results/ | head -1
