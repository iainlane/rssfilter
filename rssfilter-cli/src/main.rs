#[cfg(not(target_arch = "wasm32"))]
use std::error::Error;

#[cfg(not(target_arch = "wasm32"))]
mod rssfilter;

#[cfg(not(target_arch = "wasm32"))]
#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    rssfilter::main().await
}

#[cfg(target_arch = "wasm32")]
fn main() {
    panic!("This application is not intended to run in a WebAssembly environment.");
}
