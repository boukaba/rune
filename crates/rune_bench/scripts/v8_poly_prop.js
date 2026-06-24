const start = performance.now();
// 10 shapes cycled via 1000 element array, 1M total accesses
var objs = [];
var i = 0;
while (i < 10) {
    var o = {};
    o["k" + i] = i;
    o.x = i;
    objs.push(o);
    i = i + 1;
}
// Duplicate to 1000 objects (cycles 10 shapes)
var j = 10;
while (j < 1000) {
    objs.push(objs[j - 10]);
    j = j + 1;
}
var s = 0;
var k = 0;
while (k < 1000) {
    var t = 0;
    while (t < 1000) {
        s = s + objs[t].x;
        t = t + 1;
    }
    k = k + 1;
}
console.log(`poly_prop_10shapes_1M: ${(performance.now() - start).toFixed(2)}ms`);
