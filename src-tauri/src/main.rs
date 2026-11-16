#[tokio::main]
async fn main() {
    // Not a return: dropping the runtime would wait on blocking tasks that
    // never finish.
    std::process::exit(tethys_lib::run().await)
}
