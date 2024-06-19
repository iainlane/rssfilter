import * as awsclassic from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";
import { Output } from "@pulumi/pulumi";
import * as gandi from "@pulumiverse/gandi";

const apiKey = pulumi.secret(
  awsclassic.ssm
    .getParameter({
      name: "/lambda-rssfilter/gandi-key",
      withDecryption: true,
    })
    .then((result) => result.value),
);

const gandiClient = new gandi.Provider("gandi", {
  key: apiKey,
});

export interface CreatedResources {
  certificate: awsclassic.acm.Certificate;
  certificateValidation: awsclassic.acm.CertificateValidation;
}

export function validatedCertificate(
  subdomain: string,
  zone: string,
): CreatedResources {
  const domainNameFull = `${subdomain}.${zone}`;

  const certificate = new awsclassic.acm.Certificate(`${domainNameFull}-cert`, {
    domainName: domainNameFull,
    validationMethod: "DNS",
  });

  const validationOptions = {
    fqdn: certificate.domainValidationOptions[0].resourceRecordName,
    name: certificate.domainValidationOptions[0].resourceRecordName.apply(
      // strip `${domainName}.` from the end: Gandi's API expects just the
      // subdomain part
      (name) => name.substring(0, name.length - `${zone}.`.length - 1),
    ),
    type: certificate.domainValidationOptions[0].resourceRecordType,
    values: [certificate.domainValidationOptions[0].resourceRecordValue],
  };

  const certificateValidationRecord = new gandi.livedns.Record(
    `${domainNameFull}-cert-validation`,
    {
      ...validationOptions,
      zone,
      ttl: 300,
    },
    {
      provider: gandiClient,
    },
  );

  const certificateValidation = new awsclassic.acm.CertificateValidation(
    "certificateValidation",
    {
      certificateArn: certificate.arn,
      validationRecordFqdns: [validationOptions.fqdn],
    },
    {
      dependsOn: certificateValidationRecord,
    },
  );

  return {
    certificate,
    certificateValidation,
  };
}

export function cnameRecord(
  subdomain: string,
  zone: string,
  target: Output<string> | string,
) {
  return new gandi.livedns.Record(
    `${subdomain}-cname`,
    {
      zone,
      ttl: 300,
      name: subdomain,
      type: "CNAME",
      values: [pulumi.interpolate`${target}.`],
    },
    {
      provider: gandiClient,
    },
  );
}
