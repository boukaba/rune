.PHONY: bench
bench:
	cargo bench -p rune_bench 2>&1 | tee crates/rune_bench/results/$$(date +%Y%m%d).txt
