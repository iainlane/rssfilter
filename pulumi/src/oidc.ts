/**
 * Configuration for OIDC, for pushing from gitHub Actions.
 */

import * as aws from "@pulumi/aws-native";
import * as pulumi from "@pulumi/pulumi";

import { gitHubRepo } from "./config";

const accountId = (await aws.getAccountId()).accountId;
const coreStack = new pulumi.StackReference(
  "organization/lambda-rssfilter-core/shared",
);
const region = (await aws.getRegion()).region;
const stack = pulumi.getStack();

const oidcAudience = "token.actions.githubusercontent.com";

const oidcProvider = coreStack
  .getOutput("oidcProviderArn")
  .apply(async (arn: string) => aws.iam.getOidcProvider({ arn }));

// A map of the audience to the client ID, used in the role's trust policy to
// ensure that only this OIDC provider can assume the role.
const clientIds = coreStack.getOutput("clientIds");

const audience = clientIds.apply((ids: { [key: string]: string }) => ({
  [`${oidcAudience}:aud`]: ids[stack],
}));

/**
 * The role that Actions workflows will assume
 */
const oidcRole = new aws.iam.Role("oidcRole", {
  assumeRolePolicyDocument: {
    Version: "2012-10-17",
    Statement: [
      {
        Effect: "Allow",
        Principal: {
          Federated: oidcProvider.arn,
        },
        Action: "sts:AssumeRoleWithWebIdentity",
        Condition: {
          StringEquals: audience.apply((audience) => ({
            ...audience,
            [`${oidcAudience}:sub`]: [
              `repo:${gitHubRepo}:pull_request`,
              `repo:${gitHubRepo}:ref:refs/heads/main`,
            ],
          })),
        },
      },
    ],
  },
});

const stateBucket = await aws.s3.getBucket({
  bucketName: `pulumi-state-${accountId}`,
});

const stateBucketKeyAlias = aws.kms.Alias.get(
  "stateBucketKey",
  "alias/pulumi-state",
);
const stateBucketKey = aws.kms.Key.get(
  "stateBucketKey",
  stateBucketKeyAlias.targetKeyId,
  {
    ignoreChanges: [
      "bypassPolicyLockoutSafetyCheck",
      "pendingWindowInDays",
      "rotationPeriodInDays",
    ],
  },
);

export function createOidcPullRequestPolicies() {
  return [
    // read from and write to the state bucket
    new aws.iam.ManagedPolicy("stateBucketPolicy", {
      description: "Allow read/write to the Pulumi state bucket",
      policyDocument: {
        Version: "2012-10-17",
        Statement: [
          {
            Effect: "Allow",
            Action: ["s3:ListBucket", "s3:GetBucketLocation"],
            Resource: stateBucket.arn,
          },
          {
            Effect: "Allow",
            Action: ["s3:DeleteObject", "s3:GetObject", "s3:PutObject"],
            Resource: `${stateBucket.arn}/*`,
          },
          // Allow read access to the KMS key used to encrypt the Pulumi state bucket
          {
            Effect: "Allow",
            Action: ["kms:Decrypt"],
            Resource: stateBucketKey.arn,
          },
          {
            Effect: "Allow",
            Action: ["kms:DescribeKey"],
            Resource: `arn:aws:kms:${region}:${accountId}:key/*`,
          },
          {
            Effect: "Allow",
            Action: ["kms:ListAliases", "kms:ListKeys"],
            Resource: "*",
          },
          // we have a Cloudflare API token in SSM parameter store
          // Allow read access to the Cloudflare API token"
          {
            Effect: "Allow",
            Action: ["ssm:GetParameter"],
            Resource: `arn:aws:ssm:${region}:${accountId}:parameter/lambda-rssfilter/cloudflare-token`,
          },
          // Allow read access to IAM roles
          {
            Effect: "Allow",
            Action: ["iam:GetRolePolicy"],
            Resource: "*",
          },
          // Allow read access to the OIDC provider"
          {
            Effect: "Allow",
            Action: ["iam:GetOpenIDConnectProvider"],
            Resource: oidcProvider.arn,
          },
          // pulumi uses the cloud control api to execute changes
          {
            Effect: "Allow",
            Action: ["cloudformation:GetResource"],
            Resource: "*",
          },
        ],
      },
      roles: [oidcRole.id],
    }),
  ];
}

export const oidc = {
  audience: oidcProvider.clientIdList,
  oidcProviderArn: oidcProvider.arn,
  roleArn: oidcRole.arn,
};
