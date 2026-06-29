function handler(requestBody) {
    var data = JSON.parse(requestBody);
    var items = data.items;

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

    // Use find to look up a specific item
    var target = items.find(function(x) { return x.name === "item999"; });
    if (target.value !== 999) { throw new Error("find failed: " + target.value); }

    // Use some to verify at least one item has value > 900
    if (!items.some(function(x) { return x.value > 900; })) {
        throw new Error("some failed: expected at least one > 900");
    }

    // Use every to verify all active items have non-negative values
    if (!items.every(function(x) { return x.value >= 0; })) {
        throw new Error("every failed: expected all non-negative");
    }

    // Use includes to verify a known value is present
    var topNames = items.slice(0, 3).map(function(x) { return x.name; });
    if (topNames.indexOf("item0") === -1) { throw new Error("includes check failed"); }

    // Use replace to sanitize body (normalize newlines, empty → default)
    var cleaned = requestBody.replace("\n", "").replace("", "{\"default\":true}");
    if (cleaned.indexOf("default") === -1) { throw new Error("replace failed"); }

    var active = items.filter(function(x) { return x.active; });

    // Use flatMap to transform top active items into rank-value pairs
    var rankValues = active.slice(0, 3).flatMap(function(x) { return [x.value * 2, x.value * 3]; });
    if (rankValues.length !== 6) { throw new Error("flatMap length mismatch: " + rankValues.length); }

    // Use sort on item names (default lexicographic)
    var names = items.map(function(x) { return x.name; }).sort();
    if (names[0] !== "item0") { throw new Error("sort failed: first should be item0"); }
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
