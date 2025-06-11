# rssfilter 📰🔍

Have you ever wanted to filter an RSS feed? No? Well, I have. 🙋‍♂️

I subscribe to some "Planet" feeds, which are aggregations of blog posts from
multiple authors. Sometimes you stumble upon contributors whose content just
doesn't click with you. Wouldn't it be great if you could enjoy everyone else's
posts without the clutter? 🤔

That's what this project does! 🎉 It takes an RSS feed and filters out items
based on your preferences. Filter on:

- The title: Dodge specific names or keywords like a pro 📝
- The link: Swerve certain domains altogether 🔗
- The GUID: For when you need to get super specific with those pesky permalinks
  🆔

Ready to take control of your RSS feed? Let's go!

## Usage

An instance of `rssfilter` is running on
`https://rssfilter.orangesquash.org.uk/`. This is public for anyone to use.

To use it, supply query parameters specifying the feed you want to filter and
the filters you want to apply. Posts matching those filters will be excluded.
Stick the URL in your feed reader and enjoy the peace and quiet 🍵.

The query parameters are:

- `url`: The URL of the feed you want to filter.
- `title_filter_regex`: A regular expression to filter the title of each item.
- `link_filter_regex`: A regular expression to filter the link of each item.
- `guid_filter_regex`: A regular expression to filter the GUID of each item.

All query parameters should be URL-encoded. The `url` and at least one filter
are required. Each of the filters can be given multiple times to filter on
multiple values.

For example, the url

```
https://rssfilter.orangesquash.org.uk/?url=https%3A%2F%2Fplanet.ubuntu.com%2Frss20.xml&link_filter_regex=https%3A%2F%2Fubuntu.com%2F%2Fblog
```

Will filter the Ubuntu Planet feed to exclude items from the official Ubuntu
blog.

## Running the project yourself

There are two ways to run this project.

### `workers-rssfilter`

`workers-rssfilter` is a serverless function that filters an RSS feed. It's
designed to be deployed to Cloudflare Workers. The function is called over HTTP
and receives an event with query parameters as described above, and uses the
`filter-rss-feed` library to filter the feed.

#### Deploying `workers-rssfilter` to Cloudflare Workers

Run `pnpm wrangler deploy` to deploy the function to Cloudflare Workers. You
will need to have a Cloudflare account.

### `rssfilter`

This is a binary, mainly used to testing the functionality of the core library,
that you can run on your own machine. It takes an RSS feed URL and a list of
regular expressions to filter out items. It will print the filtered feed to
stdout. Why is that useful beyond testing? No idea. It would be better if it
rendered the feed for the console or something.

### Usage

```console
$ rssfilter --help
rss_filter 0.1.0

USAGE:
    rssfilter [FLAGS] [OPTIONS] <url>

FLAGS:
    -d, --debug
    -h, --help       Prints help information
    -V, --version    Prints version information

OPTIONS:
    -g, --guid-filter-regex <guid-filter-regex>
    -l, --link-filter-regex <link-filter-regex>
    -t, --title-filter-regex <title-filter-regex>

ARGS:
    <url>
```
