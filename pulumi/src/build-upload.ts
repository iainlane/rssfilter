import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws-native";
import * as awsclassic from "@pulumi/aws";
import { Mime } from "mime";
import { walk } from "./walk";

import { ZippedRustBinary } from "./build-rust-lambda";

const zippedLambda = new ZippedRustBinary("lambda-rssfilter", {
  directory: "..",
  packageName: "lambda-rssfilter",
  target: "aarch64-unknown-linux-gnu",
});

/* S3 bucket to store the zipped lambda.
 *
 * No need to give the region. From [the docs][s3-docs]:
 * > The AWS::S3::Bucket resource creates an Amazon S3 bucket in the same AWS
 * > Region where you create the AWS CloudFormation stack.
 *
 * [s3-docs]: https://www.pulumi.com/registry/packages/aws-native/api-docs/s3/bucket/
 */
const bucket = new aws.s3.Bucket("lambda-rssfilter", {
  ownershipControls: {
    rules: [
      {
        objectOwnership:
          aws.s3.BucketOwnershipControlsRuleObjectOwnership
            .BucketOwnerPreferred,
      },
    ],
  },
  publicAccessBlockConfiguration: {
    blockPublicAcls: true,
    blockPublicPolicy: true,
    ignorePublicAcls: true,
    restrictPublicBuckets: true,
  },
  versioningConfiguration: {
    status: aws.s3.BucketVersioningConfigurationStatus.Enabled,
  },
});

export const bucketName = pulumi.interpolate`${bucket.bucketName}`;

// Upload the `zipData` to the bucket, prefixed by the stack
const rssFilterZipObject = new awsclassic.s3.BucketObjectv2(
  "lambda-rssfilter-zip",
  {
    bucket: bucketName,
    key: "lambda-rssfilter.zip",
    contentType: "application/zip",
    contentBase64: zippedLambda.zipData,
  },
);

async function syncDirectoryToS3Bucket(
  name: string,
  prefix: string,
): Promise<void> {
  const mime = new Mime();

  return walk(name, (filePath, stats) => {
    const relativePath = filePath.replace(name, "");
    const object = new awsclassic.s3.BucketObject(relativePath, {
      bucket: bucketName,
      key: `${prefix}/${relativePath}`,
      source: new pulumi.asset.FileAsset(filePath),
      contentType: mime.getType(filePath) ?? undefined,
    });

    return undefined;
  });
}

await syncDirectoryToS3Bucket("../static/", "static");

export const key = rssFilterZipObject.key;
export const versionId = rssFilterZipObject.versionId;
