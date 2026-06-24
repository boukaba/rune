const start = performance.now();
function add(a, b) {
    return a + b;
}
var s = 0;
for (var i = 0; i < 1000000; i = i + 1) {
    s = add(s, i);
}
console.log(`jit_hot_function_1M: ${(performance.now() - start).toFixed(2)}ms`);
