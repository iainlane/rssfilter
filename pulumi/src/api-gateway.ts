import * as aws from "@pulumi/aws-native";
import * as awsclassic from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";

import type { CreatedResources as LambdaResources } from "./lambda";

export interface CreatedResources {
  apiGateway: awsclassic.apigatewayv2.Api;
  apiGatewayIntegration: awsclassic.apigatewayv2.Integration;
  apiGatewayRoute: awsclassic.apigatewayv2.Route;
  apiGatewayStage: awsclassic.apigatewayv2.Stage;
}

export function createApiGateway(
  name: string,
  { lambda }: LambdaResources,
): CreatedResources & { targetUrl: pulumi.Output<string> } {
  const apiGateway = new awsclassic.apigatewayv2.Api(name, {
    protocolType: "HTTP",
    disableExecuteApiEndpoint: false,
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

  const targetUrl = apiGateway.apiEndpoint;

  return {
    apiGateway,
    apiGatewayIntegration: integration,
    apiGatewayRoute: route,
    apiGatewayStage: stage,
    targetUrl,
  };
}
