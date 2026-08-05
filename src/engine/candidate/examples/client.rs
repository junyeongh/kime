//! Drives the candidate window standalone:
//!
//! ```sh
//! cargo run -p kime-engine-candidate --example client
//! ```
//!
//! Requires `kime-candidate-window` on `PATH`. Only mouse clicks select here --
//! keyboard handling lives in the input method, not in the renderer.

use kime_engine_candidate::client::Client;

fn main() {
    let candidate_list = kime_engine_dict::lookup("가").unwrap();
    let client = Client::new(candidate_list).unwrap();

    // `is_ready` blocks for up to 200ms per call, so this polls rather than spins.
    while !client.is_ready() {}

    match client.close() {
        Ok(Some(index)) => println!("selected {index}: {:?}", candidate_list[index]),
        Ok(None) => println!("cancelled"),
        Err(err) => eprintln!("candidate window failed: {err}"),
    }
}
