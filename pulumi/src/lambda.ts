import * as aws from "@pulumi/aws-native";
import * as awsclassic from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";
import { Output } from "@pulumi/pulumi";

export function createLambda(
  name: string,
  bucket: Output<string> | string,
  key: Output<string> | string,
  versionId: Output<string> | string,
) {
  const stack = pulumi.getStack();

  const logGroup = new aws.logs.LogGroup(name, {
    logGroupName: `/aws/lambda/lambda-rssfilter-${stack}`,
    retentionInDays: 7,
  });

  const role = new aws.iam.Role(name, {
    assumeRolePolicyDocument: awsclassic.iam.assumeRolePolicyForPrincipal({
      Service: "lambda.amazonaws.com",
    }),
  });

  const cloudWatchPolicy = new aws.iam.RolePolicy(`${name}-cloudwatch`, {
    roleName: pulumi.interpolate`${role.roleName}`,
    policyDocument: {
      Version: "2012-10-17",
      Statement: [
        {
          Effect: "Allow",
          Action: [
            "logs:CreateLogGroup",
            "logs:CreateLogStream",
            "logs:PutLogEvents",
          ],
          Resource: logGroup.arn,
        },
      ],
    },
  });

  const xrayWritePolicy = new aws.iam.RolePolicy(`${name}-xray-write`, {
    roleName: pulumi.interpolate`${role.roleName}`,
    policyDocument: {
      Version: "2012-10-17",
      Statement: [
        {
          Effect: "Allow",
          Action: ["xray:PutTraceSegments", "xray:PutTelemetryRecords"],
          Resource: "*",
        },
      ],
    },
  });

  const lambda = new aws.lambda.Function(name, {
    architectures: ["arm64"],
    code: {
      s3Bucket: bucket,
      s3Key: key,
      s3ObjectVersion: versionId,
    },
    handler: "rust.handler",
    loggingConfig: {
      applicationLogLevel: "DEBUG",
      logFormat: "JSON",
      logGroup: pulumi.interpolate`${logGroup.logGroupName}`,
    },
    packageType: "Zip",
    role: role.arn,
    runtime: "provided.al2023",
    tracingConfig: {
      mode: aws.lambda.FunctionTracingConfigMode.Active,
    },
  });

  return { lambda };
}
