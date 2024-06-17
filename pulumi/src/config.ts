import { Config } from "@pulumi/pulumi";

const c = new Config();

export const appName = "lambda-rssfilter";
export const subdomain = c.require("subdomain");
export const domainName = c.require("domainName");
export const domainNameFull = `${subdomain}.${domainName}`;
export const gitHubRepo = c.require("gitHubRepo");
