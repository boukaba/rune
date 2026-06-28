function handler(requestBody) {
    var data = JSON.parse(requestBody);
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
result.total
