import * as aws from "@pulumi/aws-native";
import * as awsclassic from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";
import { Output } from "@pulumi/pulumi";

export interface CreatedResources {
  lambda: aws.lambda.Function;
  lambdaLogGroup: aws.logs.LogGroup;
  lambdaRole: aws.iam.Role;
}

export function createLambda(
  name: string,
  bucket: aws.s3.Bucket,
  key: Output<string> | string,
  versionId: Output<string> | string,
): CreatedResources {
  const stack = pulumi.getStack();

  const lambdaLogGroup = new aws.logs.LogGroup(name, {
    logGroupName: `/aws/lambda/lambda-rssfilter-${stack}`,
    retentionInDays: 7,
  });

  const lambdaRole = new aws.iam.Role(name, {
    assumeRolePolicyDocument: awsclassic.iam.assumeRolePolicyForPrincipal({
      Service: "lambda.amazonaws.com",
    }),
  });

  const cloudWatchPolicy = new aws.iam.RolePolicy(`${name}-cloudwatch`, {
    roleName: pulumi.interpolate`${lambdaRole.roleName}`,
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
          Resource: lambdaLogGroup.arn,
        },
      ],
    },
  });

  const xrayWritePolicy = new aws.iam.RolePolicy(`${name}-xray-write`, {
    roleName: pulumi.interpolate`${lambdaRole.roleName}`,
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
      s3Bucket: pulumi.interpolate`${bucket.bucketName}`,
      s3Key: key,
      s3ObjectVersion: versionId,
    },
    handler: "rust.handler",
    loggingConfig: {
      applicationLogLevel: "DEBUG",
      logFormat: "JSON",
      logGroup: pulumi.interpolate`${lambdaLogGroup.logGroupName}`,
    },
    packageType: "Zip",
    role: lambdaRole.arn,
    runtime: "provided.al2023",
    tracingConfig: {
      mode: aws.lambda.FunctionTracingConfigMode.Active,
    },
  });

  return { lambda, lambdaLogGroup, lambdaRole };
}
