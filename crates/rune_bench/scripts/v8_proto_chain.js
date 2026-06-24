const start = performance.now();
function mk(level) {
    if (level === 0) {
        return { x: 42 };
    }
    var o = {};
    Object.setPrototypeOf(o, mk(level - 1));
    return o;
}
var o = mk(5);
var s = 0;
for (var i = 0; i < 1000000; i = i + 1) {
    s = s + o.x;
}
console.log(`proto_chain_lookup_5deep_1M: ${(performance.now() - start).toFixed(2)}ms`);
