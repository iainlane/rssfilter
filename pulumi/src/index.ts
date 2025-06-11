/**
 * Pulumi program to manage some Cloudflare resources for `workers-rssfilter`.
 *
 * - DNS records are created in Cloudflare with proxy enabled.
 * - TLS certificates are managed by Cloudflare.
 */

import { domainName, subdomain } from "./config";
import { cloudflare } from "./dns-tls";
import {
  createOidcPullRequestPolicies as createOIDCPolicies,
  oidc,
} from "./oidc";

cloudflare(subdomain, domainName);
createOIDCPolicies();

export const fqdn = `https://${subdomain}.${domainName}`;
export { oidc };
