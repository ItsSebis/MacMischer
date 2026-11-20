//! Read-only hardware check: connect, time the full sync, count meter updates.
//! `cargo run -p xair-core --example sync_check -- 10.5.5.48`

use std::time::{Duration, Instant};
use xair_core::{Mixer, Status};
use xair_proto::{Param, StripId};

fn main() {
    let ip = std::env::args().nth(1).unwrap_or_else(|| "10.5.5.48".into());
    let t0 = Instant::now();
    let mixer = Mixer::connect(format!("{ip}:10024").parse().unwrap());
    while mixer.status() != Status::Connected {
        if t0.elapsed() > Duration::from_secs(15) {
            panic!("no full sync after 15 s: {:?}", mixer.status());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let st = mixer.state();
    println!("synced {} / {} params in {:?}", st.values.len(), Param::sync_set().len(), t0.elapsed());
    println!("{:?}", st.info);
    let missing: Vec<String> = Param::sync_set().into_iter().filter(|p| st.get(*p).is_none()).map(|p| p.path()).collect();
    println!("missing: {missing:?}");
    for s in StripId::all() {
        print!("{}={:.1}dB{} ", st.name(s), st.fader_db(s), if st.on(s) { "" } else { "(M)" });
    }
    println!("\nsnapshot {} {:?}", st.snap_index(), st.snap_name(st.snap_index() as u8));
    drop(st);
    let mut n = 0;
    let mut last = None;
    let t1 = Instant::now();
    while t1.elapsed() < Duration::from_secs(3) {
        let u = mixer.meters().updated;
        if u != last { n += 1; last = u; }
        std::thread::sleep(Duration::from_millis(2));
    }
    println!("meter updates in 3 s: {n}; status {:?}", mixer.status());
}
