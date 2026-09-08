use stool::formats::pickle;

fn main() {
    let mut data = vec![0x80u8, 2, b'}'];
    let name = b"img/a.png";
    data.push(b'X');
    data.extend_from_slice(&(name.len() as u32).to_le_bytes());
    data.extend_from_slice(name);
    data.push(b']');
    for v in [10u32, 20] {
        data.push(b'J');
        data.extend_from_slice(&v.to_le_bytes());
    }
    data.push(0x86); // TUPLE2
    data.push(b'a'); // APPEND
    data.push(b's');
    data.push(b'.');
    match pickle::loads(&data) {
        Ok(v) => println!("ok: {:?}", v),
        Err(e) => println!("err: {}", e),
    }
}
