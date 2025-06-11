/**
 * Configuration for OIDC, for pushing from gitHub Actions.
 */

import * as aws from "@pulumi/aws-native";
import * as pulumi from "@pulumi/pulumi";

import type { CreatedResources as APIGatewayResources } from "./api-gateway";
import type { CreatedResources as BuildUploadResources } from "./build-upload";
import type { CreatedResources as LambdaResources } from "./lambda";

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
 * The role that Actions workflows will assume when running for pull requests.
 */
const oidcPullRequestRole = new aws.iam.Role("oidcPullRequestRole", {
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
            [`${oidcAudience}:sub`]: `repo:${gitHubRepo}:pull_request`,
          })),
        },
      },
    ],
  },
});

/**
 * The role that Actions workflows will assume when running for pushes.
 */
const oidcPushRole = new aws.iam.Role("oidcPushRole", {
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
            [`${oidcAudience}:sub`]: `repo:${gitHubRepo}:ref:refs/heads/main`,
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

export function createOidcPullRequestPolicies({ lambda }: LambdaResources) {
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
          // pulumi uses the cloud control api to execute changes
          {
            Effect: "Allow",
            Action: ["cloudformation:GetResource"],
            Resource: "*",
          },
          // Allow read access to the lambda function"
          {
            Effect: "Allow",
            Action: ["lambda:GetFunction"],
            Resource: lambda.arn,
          },
          // Allow read access to IAM roles"
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
        ],
      },
      roles: [oidcPullRequestRole.id, oidcPushRole.id],
    }),
  ];
}

// policies for only the push role. It can do everything the pull request role
// can do, because it's included in the managed policies above. But it can also
// create-update-delete the resources themselves. IOW, we can preview for PRs
// and apply for pushes.
export function createOidcPushPolicies(
  resources: LambdaResources & BuildUploadResources,
): aws.iam.ManagedPolicy {
  const { storageBucket, lambda } = resources;

  return new aws.iam.ManagedPolicy("createPolicy", {
    description: "Allow GitHub actions to bootstrap resources",
    policyDocument: {
      Version: "2012-10-17",
      Statement: [
        // Pulumi uses the Cloud Control API to execute changes
        {
          Effect: "Allow",
          Action: ["cloudformation:*"],
          Resource: `arn:aws:cloudformation:${region}:*:resource/*`,
        },
        // Also needs to be able to update the storage bucket
        {
          Effect: "Allow",
          Action: ["s3:ListBucket", "s3:GetBucketLocation"],
          Resource: storageBucket.arn,
        },
        {
          Effect: "Allow",
          Action: [
            "s3:DeleteObject",
            "s3:GetObject",
            "s3:GetObjectTagging",
            "s3:GetObjectVersion",
            "s3:PutObject",
          ],
          Resource: pulumi.interpolate`${storageBucket.arn}/*`,
        },
        {
          Effect: "Allow",
          Action: ["lambda:UpdateFunctionCode"],
          Resource: lambda.arn,
        },
      ],
    },
    roles: [oidcPushRole.id],
  });
}

export const oidc = {
  audience: oidcProvider.clientIdList,
  oidcProviderArn: oidcProvider.arn,
  roleArns: {
    pullRequests: oidcPullRequestRole.arn,
    pushes: oidcPushRole.arn,
  },
};
