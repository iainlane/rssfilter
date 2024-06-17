use log::info;
use regex::Regex;
use std::env;
use std::error::Error;
use structopt::StructOpt;

use filter_rss_feed::{FilterRegexes, RssFilter};

#[derive(StructOpt, Debug)]
#[structopt(name = "rss_filter")]
struct Opt {
    #[structopt(short, long)]
    title_filter_regex: Option<String>,

    #[structopt(short, long)]
    guid_filter_regex: Option<String>,

    #[structopt(short, long)]
    link_filter_regex: Option<String>,

    #[structopt(short, long)]
    debug: bool,

    #[structopt()]
    url: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let opt = Opt::from_args();

    if opt.debug {
        env::set_var("RUST_LOG", "debug");
    }
    env_logger::init();

    info!("Starting RSS filter application");

    let title_regex = opt
        .title_filter_regex
        .as_deref()
        .map(Regex::new)
        .transpose()?;
    let guid_regex = opt
        .guid_filter_regex
        .as_deref()
        .map(Regex::new)
        .transpose()?;
    let link_regex = opt
        .link_filter_regex
        .as_deref()
        .map(Regex::new)
        .transpose()?;

    let rss_filter = RssFilter::new(FilterRegexes {
        title_regex,
        guid_regex,
        link_regex,
    });

    match rss_filter?.fetch_and_filter(&opt.url).await {
        Ok(channel) => {
            println!("{}", channel);
            Ok(())
        }
        Err(e) => Err(e),
    }
}
