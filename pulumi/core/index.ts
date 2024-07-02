import * as pulumi from "@pulumi/pulumi";
import * as aws from "@pulumi/aws-native";

const projectName = pulumi.getProject();
const oidcAudience = "token.actions.githubusercontent.com";

const clientIds = Object.fromEntries(
  ["dev", "prod"].map((stage) => [stage, `${projectName}-${stage}`]),
);

const oidcProvider: aws.iam.OidcProvider = new aws.iam.OidcProvider(
  "gitHub-oidc",
  {
    clientIdList: Object.values(clientIds),
    thumbprintList: [
      // gitHub's thumbprints as of 2024-06-06. According to AWS's documentation,
      // these aren't used for validation.
      "6938fd4d98bab03faadb97b34396831e3780aea1",
      "1c58a3a8518e8759bf075b76b750d4f2df264fcd",
    ],
    url: `https://${oidcAudience}`,
  },
);

export { clientIds };
export const oidcProviderArn = oidcProvider.arn;
