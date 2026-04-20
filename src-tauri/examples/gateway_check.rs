fn main() {
    match rubypass_lib::gateway::detect() {
        Ok(gw) => println!("Gateway: {}", gw),
        Err(e) => println!("Error: {}", e),
    }
}
