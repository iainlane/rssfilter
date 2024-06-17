# rust-filter-rss

## Bootstraping the project

### Make sure the AWS CLI is configured

We suggest using AWS SSO for this. Your configuration will be mounted in the dev
container.

If you have multiple profiles and aren't using `us-east-1`, create a
`.devcontainer/.env` file and set `AWS_PROFILE` and `AWS_REGION` accordingly.

To verify that the AWS CLI is configured correctly, run:

```console
$ aws sts get-caller-identity
```

If this outputs your account ID, you're good to go.

### Set up Pulumi

Pulumi needs somewhere to store its state. One options is storing its state
remotely in an S3 bucket. This is useful just to avoid having state locally
where it can more easily be lost.

Still, you can use the default local state storage if you prefer. Do it like
this:

```console
$ pulumi login --local
```

To create an S3 bucket to use, do this:

```console
$ aws s3api create-bucket \
  --bucket "pulumi-state-$(aws sts get-caller-identity --query Account --output text)" \
  --create-bucket-configuration "LocationConstraint=$(aws ec2 describe-availability-zones --output text --query 'AvailabilityZones[0].[RegionName]')"
{
    "Location": "http://pulumi-state-12345789.s3.amazonaws.com/"
}
```

This will output a bucket name. It's the bit before ".s3.amazonaws.com" and
after "http://". In the above output, it's `pulumi-state-12345789`.

Now, put that URL in a environment variable called `PULUMI_BACKEND_URL` in
`.devcontainer/.env`:

```
s3://<bucket_name>?awssdk=v2
```

and make sure it's available in the current session:

```console
$ . .devcontainer/.env
$ export PULUMI_BACKEND_URL
```

Now you can log in to Pulumi:

```console
$ pulumi login $PULUMI_BACKEND_URL
Logged in to <some ID> as <username> (s3://<bucket_name>?awssdk=v2)
```

Next we need a way to encrypt the state for the stack. With Pulumi you can use a
passphrase or a KMS key (there are other options too, but since we're on AWS
these are the relevant two). We'll go with KMS for now. But be aware there is a
charge for this: KMS keys have no free tier.

We also create an "alias" so we can refer to the key by name. If you pick the
same name we do here, the Pulumi configuration in the repository will work
without modification.

````console

To create a KMS key, do this:

```console
$ aws kms create-key --description "Pulumi state encryption key"
{
    "KeyMetadata": {
        ...
        "Arn": "arn:aws:kms:REGION:ACCOUNT:key/ID",
        ...
}
$ aws kms create-alias --alias-name "alias/pulumi-state" --target-key-id "arn:aws:kms:REGION:ACCOUNT:key/ID"
````

#### Get a Gandi API Key

We host our domain on Gandi. To provision the infrastructure, we'll need to be
able to manage the DNS records. To do this, we need an API key.

Visit [Gandi's API key page][api-key-page] and create a new API key ([currently
this needs to be an API Key and not a Personal Access Token][api-key-issue]).
You'll need to give it a name and select the permissions to manage DNS records.

Copy the key and upload to the AWS SSM Parameter Store:

```console
$ aws ssm put-parameter --name /lambda-rssfilter/gandi-key --value "<your key>" --type SecureString
```

this will be encrypted by AWS.

[api-key-issue]: https://github.com/pulumiverse/pulumi-gandi/issues/3
[api-key-page]: https://account.gandi.net/en/users/security

### Set configuration variables

Some configuration variables are set in [`Pulumi.yaml`][yaml].

[yaml]: ./pulumi/Pulumi.yaml
