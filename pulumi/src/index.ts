/**
 * Pulumi program to build and deploy the `lambda-rssfilter` program.
 *
 * - DNS records are created in Gandi.
 * - TLS certificates are created in AWS Certificate Manager.
 * - The application is deployed as a Lambda function behind an API Gateway.
 * - An OIDC provider is created for GitHub Actions to assume roles, along with
 *   the roles for Actions workflows to manage the deployment.
 */

import { apiGateway } from "./api-gateway";
import { bucketName, key, versionId } from "./build-upload";
import { appName, domainName, subdomain } from "./config";
import { cnameRecord, validatedCertificate } from "./dns-tls";
import { createLambda } from "./lambda";

const certificate = validatedCertificate(subdomain, domainName);
const { lambda } = createLambda(appName, bucketName, key, versionId);
const { targetUrl } = apiGateway(appName, lambda, certificate);
cnameRecord(subdomain, domainName, targetUrl);

export * from "./oidc";
export const fqdn = `https://${subdomain}.${domainName}`;
