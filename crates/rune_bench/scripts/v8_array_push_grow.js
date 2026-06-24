const start = performance.now();
var a = [];
for (var i = 0; i < 100000; i = i + 1) {
    a.push(i);
}
console.log(`array_push_grow_100k: ${(performance.now() - start).toFixed(2)}ms (len=${a.length})`);
