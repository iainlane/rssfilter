/**
 * Configuration for OIDC, for pushing from gitHub Actions.
 */

import * as aws from "@pulumi/aws-native";
import * as pulumi from "@pulumi/pulumi";

import type { CreatedResources as APIGatewayResources } from "./api-gateway";
import type { CreatedResources as BuildUploadResources } from "./build-upload";
import type { CreatedResources as DNSTLSResources } from "./dns-tls";
import type { CreatedResources as LambdaResources } from "./lambda";

import { gitHubRepo } from "./config";
import { Resource } from "@pulumi/aws-native/apigateway";

const accountId = (await aws.getAccountId()).accountId;
const region = (await aws.getRegion()).region;

const projectName = pulumi.getProject();
const stack = pulumi.getStack();

const oidcAudience = "token.actions.githubusercontent.com";
const oidcProvider = new aws.iam.OidcProvider("gitHub-oidc", {
  clientIdList: [`${projectName}-${stack}`],
  thumbprintList: [
    // gitHub's thumbprints as of 2024-06-06. According to AWS's documentation,
    // these aren't used for validation.
    "6938fd4d98bab03faadb97b34396831e3780aea1",
    "1c58a3a8518e8759bf075b76b750d4f2df264fcd",
  ],
  url: `https://${oidcAudience}`,
});

// A map of the audience to the client ID, used in the role's trust policy to
// ensure that only this OIDC provider can assume the role.
const audiences = oidcProvider.clientIdList.apply(
  (ids) =>
    ids && Object.fromEntries(ids.map((id) => [`${oidcAudience}:aud`, id])),
);

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
          StringEquals: audiences.apply((audiences) => ({
            ...audiences,
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
const oidcPush = new aws.iam.Role("oidcPushRole", {
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
          StringEquals: audiences.apply((audiences) => ({
            ...audiences,
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
);

const oidcPullRequestPolicies = [
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
      ],
    },
    roles: [oidcPullRequestRole.id, oidcPush.id],
  }),

  // decrypt the kms key used to encrypt secrets in the state bucket
  new aws.iam.ManagedPolicy("kmsReadOnlyPolicy", {
    description:
      "Allow read access to the KMS key used to encrypt the Pulumi state bucket",
    policyDocument: {
      Version: "2012-10-17",
      Statement: [
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
      ],
    },
    roles: [oidcPullRequestRole.id, oidcPush.id],
  }),

  // we have a Gandi API key in SSM parameter store
  new aws.iam.ManagedPolicy("ssmReadOnlyPolicy", {
    description: "Allow read access to the Gandi API key",
    policyDocument: {
      Version: "2012-10-17",
      Statement: [
        {
          Effect: "Allow",
          Action: ["ssm:GetParameter"],
          Resource: `arn:aws:ssm:${region}:${accountId}:parameter/lambda-rssfilter/gandi-key`,
        },
      ],
    },
    roles: [oidcPullRequestRole.id, oidcPush.id],
  }),

  // pulumi uses the cloud control api to execute changes
  new aws.iam.ManagedPolicy("cloudControlGetResourcesPolicy", {
    description: "Allow read access to CloudFormation resources",
    policyDocument: {
      Version: "2012-10-17",
      Statement: [
        {
          Effect: "Allow",
          Action: ["cloudformation:GetResource"],
          Resource: "*",
        },
      ],
    },
    roles: [oidcPullRequestRole.id, oidcPush.id],
  }),
];

// policies for only the push role. It can do everything the pull request role
// can do, because it's included in the managed policies above. But it can also
// create-update-delete the resources themselves. IOW, we can preview for PRs
// and apply for pushes.

export function createOidcPushPolicies(
  storageBucket: aws.s3.Bucket,
): aws.iam.ManagedPolicy {
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
            "s3:PutObject",
          ],
          Resource: pulumi.interpolate`${storageBucket.arn}/*`,
        },
      ],
    },
    roles: [oidcPush.id],
  });
}

export const oidc = {
  audience: oidcProvider.clientIdList,
  roleArns: {
    pullRequests: oidcPullRequestRole.arn,
    pushes: oidcPush.arn,
  },
};
