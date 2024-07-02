/**
 * Pulumi program to build and deploy the `lambda-rssfilter` program.
 *
 * - DNS records are created in Gandi.
 * - TLS certificates are created in AWS Certificate Manager.
 * - The application is deployed as a Lambda function behind an API Gateway.
 * - An OIDC provider is created for GitHub Actions to assume roles, along with
 *   the roles for Actions workflows to manage the deployment.
 */

import { createApiGateway } from "./api-gateway";
import { key, storageBucket, versionId } from "./build-upload";
import { appName, domainName, subdomain } from "./config";
import { cnameRecord, validatedCertificate } from "./dns-tls";
import { createLambda } from "./lambda";
import {
  createOidcPushPolicies,
  createOidcPullRequestPolicies,
  oidc,
} from "./oidc";

const cert = validatedCertificate(subdomain, domainName);

const lambda = await createLambda(appName, storageBucket, key, versionId);

const { targetUrl } = createApiGateway(appName, lambda, cert);

cnameRecord(subdomain, domainName, targetUrl);
createOidcPullRequestPolicies(lambda);
createOidcPushPolicies({ storageBucket, ...lambda });

export const fqdn = `https://${subdomain}.${domainName}`;
export { oidc };
