const start = performance.now();
var s = 0;
for (var i = 0; i < 1000000; i = i + 1) {
    s = s + i;
}
console.log(`loop_sum_smi_1M: ${(performance.now() - start).toFixed(2)}ms`);
