function handler(requestBody) {
    var data = JSON.parse(requestBody);

    // Verify expected keys with Object.keys + direct comparison
    var fields = Object.keys(data);
    if (fields.length !== 1) { throw new Error("expected 1 field, got " + fields.length); }
    if (fields[0] !== "items") { throw new Error("expected field 'items', got " + fields[0]); }

    // Use Object.entries for metadata summary
    var entries = Object.entries(data);
    if (entries.length !== 1) { throw new Error("expected 1 entry"); }
    if (entries[0][0] !== "items") {
        throw new Error("expected entry key 'items', got " + entries[0][0]);
    }

    var active = data.items.filter(function(x) { return x.active; });
    var total = active.reduce(function(sum, x) { return sum + x.value; }, 0);
    var top = active
        .map(function(x) { return { name: x.name, value: x.value }; })
        .slice(0, 10);
    return { total: total, count: active.length, top: top };
}

var items = [];
for (var i = 0; i < 1000; i = i + 1) {
    items.push({ name: "item" + i, value: i, active: (i % 3 === 0) });
}
var input = JSON.stringify({ items: items });

var result = handler(input);
if (result.total !== 166833) { throw new Error("total mismatch: " + result.total); }
if (result.count !== 334) { throw new Error("count mismatch: " + result.count); }
if (result.top.length !== 10) { throw new Error("top length mismatch: " + result.top.length); }
result.total
