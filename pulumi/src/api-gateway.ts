import * as aws from "@pulumi/aws-native";
import * as awsclassic from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";

export interface CreatedResources {
  apiGateway: awsclassic.apigatewayv2.Api;
  apiGatewayDomain: awsclassic.apigatewayv2.DomainName;
  apiGatewayIntegration: awsclassic.apigatewayv2.Integration;
  apiGatewayRoute: awsclassic.apigatewayv2.Route;
  apiGatewayStage: awsclassic.apigatewayv2.Stage;
}

export function createApiGateway(
  name: string,
  lambda: aws.lambda.Function,
  certificate: {
    domainName: pulumi.Output<string>;
    arn: pulumi.Output<string>;
  },
): CreatedResources & { targetUrl: pulumi.Output<string> } {
  const apiGateway = new awsclassic.apigatewayv2.Api(name, {
    protocolType: "HTTP",
    disableExecuteApiEndpoint: true,
  });

  const integration = new awsclassic.apigatewayv2.Integration(
    `${name}-integration`,
    {
      apiId: apiGateway.id,
      integrationMethod: "GET",
      integrationType: "AWS_PROXY",
      integrationUri: lambda.arn,
      payloadFormatVersion: "2.0",
    },
  );

  const route = new awsclassic.apigatewayv2.Route(`${name}-route`, {
    apiId: apiGateway.id,
    routeKey: "GET /",
    target: pulumi.interpolate`integrations/${integration.id}`,
  });

  const stage = new awsclassic.apigatewayv2.Stage(`${name}-stage`, {
    name: "$default",
    apiId: apiGateway.id,
    autoDeploy: true,
  });

  const lambdaInvokePermission = new aws.lambda.Permission(
    `${name}-invoke-permission`,
    {
      action: "lambda:InvokeFunction",
      functionName: lambda.arn,
      principal: "apigateway.amazonaws.com",
      sourceArn: pulumi.interpolate`${apiGateway.executionArn}/*/*/*`,
    },
  );

  const customDomain = new awsclassic.apigatewayv2.DomainName(
    `${name}-domain`,
    {
      domainName: certificate.domainName,
      domainNameConfiguration: {
        certificateArn: certificate.arn,
        endpointType: "REGIONAL",
        securityPolicy: "TLS_1_2",
      },
    },
  );

  const mapping = new awsclassic.apigatewayv2.ApiMapping(`${name}-mapping`, {
    apiId: apiGateway.id,
    domainName: customDomain.id,
    stage: stage.id,
  });

  const targetUrl = customDomain.domainNameConfiguration.targetDomainName;

  return {
    apiGateway,
    apiGatewayDomain: customDomain,
    apiGatewayIntegration: integration,
    apiGatewayRoute: route,
    apiGatewayStage: stage,
    targetUrl,
  };
}
