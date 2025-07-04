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

    url: String,
}

pub async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let opt = Opt::from_args();

    if opt.debug {
        env::set_var("RUST_LOG", "debug");
    }
    env_logger::init();

    info!("Starting RSS filter application");

    let title_regexes = opt
        .title_filter_regex
        .as_deref()
        .map(Regex::new)
        .transpose()?
        .map(|r| vec![r]);
    let guid_regexes = opt
        .guid_filter_regex
        .as_deref()
        .map(Regex::new)
        .transpose()?
        .map(|r| vec![r]);
    let link_regexes = opt
        .link_filter_regex
        .as_deref()
        .map(Regex::new)
        .transpose()?
        .map(|r| vec![r]);

    let filter_regexes = FilterRegexes {
        title_regexes: &title_regexes.unwrap_or(vec![]),
        guid_regexes: &guid_regexes.unwrap_or(vec![]),
        link_regexes: &link_regexes.unwrap_or(vec![]),
    };

    let rss_filter = RssFilter::new(&filter_regexes)?;

    let filtered = rss_filter.fetch_and_filter(&opt.url).await?.into_body();

    let s = std::str::from_utf8(&filtered)?;
    println!("{s}");

    Ok(())
}
