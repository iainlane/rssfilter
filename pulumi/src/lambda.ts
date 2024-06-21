import * as aws from "@pulumi/aws-native";
import * as awsclassic from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";
import { Output } from "@pulumi/pulumi";

export interface CreatedResources {
  lambda: aws.lambda.Function;
  lambdaLogGroup: aws.logs.LogGroup;
  lambdaRole: aws.iam.Role;
}

export async function createLambda(
  name: string,
  bucket: aws.s3.Bucket,
  key: Output<string> | string,
  versionId: Output<string> | string,
): Promise<CreatedResources> {
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
          Action: [
            "xray:PutTraceSegments",
            "xray:PutTelemetryRecords",
            "xray:GetSamplingRules",
            "xray:GetSamplingTargets",
            "xray:GetSamplingStatisticSummaries",
          ],
          Resource: "*",
        },
      ],
    },
  });

  // To get X-Ray traces via OpenTelemetry, we need to add the AWS X-Ray SDK to the
  // Lambda function. This is done via a Lambda layer.
  //
  // See [lambda-go] for the format of the ARNs. This says it's for Go, but it's
  // really for the `provided` family of runtimes, which we use too.
  //
  // [lambda-go]: https://aws-otel.github.io/docs/getting-started/lambda/lambda-go#lambda-layer
  const region = (await aws.getRegion()).region;
  const version = "0-98-0";
  const revision = "5";

  const adotLayerARN = pulumi.interpolate`arn:aws:lambda:${region}:901920570463:layer:aws-otel-collector-arm64-ver-${version}:${revision}`;

  const lambda = new aws.lambda.Function(name, {
    architectures: ["arm64"],
    code: {
      s3Bucket: pulumi.interpolate`${bucket.bucketName}`,
      s3Key: key,
      s3ObjectVersion: versionId,
    },
    handler: "rust.handler",
    layers: [adotLayerARN],
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
