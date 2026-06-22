.PHONY: bench bench-no-jit
bench:
	cargo bench -p rune_bench 2>&1 | tee crates/rune_bench/results/$$(date +%Y%m%d).txt

bench-no-jit:
	cargo bench -p rune_bench --no-default-features 2>&1 | tee crates/rune_bench/results/$$(date +%Y%m%d)_nojit.txt
