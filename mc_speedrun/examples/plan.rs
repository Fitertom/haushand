//! Prints how to get an item: `cargo run --example plan -- minecraft:pink_terracotta 4
//! [have=minecraft:iron_pickaxe:1,minecraft:coal:8]`.

use std::collections::HashMap;

use speedrun_bot::knowledge::Knowledge;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(item) = args.first() else {
        eprintln!("usage: plan <item id> [count] [have=id:n,id:n...]");
        std::process::exit(2);
    };
    let item = if item.contains(':') { item.clone() } else { format!("minecraft:{item}") };
    let count: i32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    let mut have: HashMap<String, i32> = HashMap::new();
    for a in &args {
        if let Some(list) = a.strip_prefix("have=") {
            for e in list.split(',') {
                let (id, n) = e.rsplit_once(':').filter(|(_, n)| n.parse::<i32>().is_ok()).unwrap_or((e, "1"));
                let id = if id.contains(':') { id.to_string() } else { format!("minecraft:{id}") };
                *have.entry(id).or_insert(0) += n.parse::<i32>().unwrap_or(1);
            }
        }
    }
    let path = std::env::var("KNOWLEDGE").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/data/knowledge.json").to_string());
    let t = std::time::Instant::now();
    let k = Knowledge::load(&path).unwrap_or_else(|e| panic!("{e}"));
    let loaded = t.elapsed();
    let t = std::time::Instant::now();
    let plan = k.plan(&item, count, &have);
    println!("{item} x{count}: {:.0} s estimated (load {:.0?}, plan {:.0?})", plan.total_cost, loaded, t.elapsed());
    print!("{}", plan.render());
    if !plan.leftover.is_empty() {
        let l: Vec<String> = plan.leftover.iter().map(|(i, n)| format!("{}:{n}", i.trim_start_matches("minecraft:"))).collect();
        println!("left over: {}", l.join(", "));
    }
}
