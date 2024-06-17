use mockito::ServerGuard;
use rss::{ChannelBuilder, GuidBuilder, Item, ItemBuilder};

type BoxError = Box<dyn std::error::Error + Send + Sync>;

fn create_items<T: AsRef<str>>(items: &[T]) -> Vec<Item> {
    items
        .iter()
        .map(|i| {
            let title = format!("Test Item {}", i.as_ref());
            let link = format!("http://www.example.com/test{}", i.as_ref());

            ItemBuilder::default()
                .title(title)
                .link(link)
                .guid(GuidBuilder::default().value(i.as_ref().to_string()).build())
                .build()
        })
        .collect()
}

fn create_test_rss_feed(items: Vec<Item>) -> Result<Vec<u8>, BoxError> {
    let bytes = vec![];

    ChannelBuilder::default()
        .title("Test RSS Feed")
        .link("http://www.example.com/")
        .description("This is a test RSS feed")
        .items(items)
        .build()
        .write_to(bytes)
        .map_err(|e| e.into())
}

pub async fn serve_test_rss_feed<T: AsRef<str>>(items: &[T]) -> Result<ServerGuard, BoxError> {
    let mut server = mockito::Server::new_async().await;

    server
        .mock("GET", "/")
        .with_status(200)
        .with_header("content-type", "application/rss+xml")
        .with_body(create_test_rss_feed(create_items(items))?.as_slice())
        .create_async()
        .await;

    Ok(server)
}
