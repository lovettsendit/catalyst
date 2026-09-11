#![crate_type = "lib"]
#[no_mangle]
pub extern "C" fn heat(x: f64, y: f64) -> f64 {
    let adjusted = if y > 1.0 { y * 0.8 } else { y };
    x * adjusted.exp()
}
#[no_mangle]
pub extern "C" fn cost(x: f64, n: i64) -> f64 {
    let mut total = 0.0;
    let mut i = 0;
    while i < n {
        total += (x * i as f64).sin();
        i += 1;
    }
    total
}
