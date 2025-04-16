import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws-native";
import * as awsclassic from "@pulumi/aws";
import { Mime } from "mime";
import { walk } from "./walk";

export interface CreatedResources {
  storageBucket: aws.s3.Bucket;
}

/* S3 bucket to store the zipped lambda.
 *
 * No need to give the region. From [the docs][s3-docs]:
 * > The AWS::S3::Bucket resource creates an Amazon S3 bucket in the same AWS
 * > Region where you create the AWS CloudFormation stack.
 *
 * [s3-docs]: https://www.pulumi.com/registry/packages/aws-native/api-docs/s3/bucket/
 */
export const storageBucket = new aws.s3.Bucket("lambda-rssfilter", {
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

const bucketName = pulumi.interpolate`${storageBucket.bucketName}`;

// Upload the pre-built zip file to the bucket
const rssFilterZipObject = new awsclassic.s3.BucketObjectv2(
  "lambda-rssfilter-zip",
  {
    bucket: bucketName,
    key: "lambda-rssfilter.zip",
    contentType: "application/zip",
    source: new pulumi.asset.FileAsset("./dist/lambda-rssfilter.zip"),
  },
);

async function syncDirectoryToS3Bucket(
  name: string,
  prefix: string,
): Promise<void> {
  const mime = new Mime();

  return walk(name, (filePath) => {
    const relativePath = filePath.replace(name, "");

    new awsclassic.s3.BucketObject(relativePath, {
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
