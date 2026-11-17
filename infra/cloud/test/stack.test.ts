import test from "node:test";
import assert from "node:assert/strict";
import { App } from "aws-cdk-lib";
import { Template, Match } from "aws-cdk-lib/assertions";
import { CloudReadinessStack } from "../src/stack";
test("synthesis cannot activate runtimes, move data or expose buckets", () => {
  const template = Template.fromStack(
    new CloudReadinessStack(new App(), "Fixture"),
  );
  for (const type of [
    "AWS::ECS::Service",
    "AWS::EC2::Instance",
    "AWS::RDS::DBInstance",
    "AWS::RDS::DBCluster",
    "AWS::SecretsManager::Secret",
    "AWS::ElasticLoadBalancingV2::LoadBalancer",
    "AWS::Route53::RecordSet",
  ])
    template.resourceCountIs(type, 0);
  template.hasResourceProperties("AWS::S3::Bucket", {
    VersioningConfiguration: { Status: "Enabled" },
    PublicAccessBlockConfiguration: {
      BlockPublicAcls: true,
      BlockPublicPolicy: true,
      IgnorePublicAcls: true,
      RestrictPublicBuckets: true,
    },
    OwnershipControls: { Rules: [{ ObjectOwnership: "BucketOwnerEnforced" }] },
  });
  template.hasResource("AWS::S3::Bucket", {
    DeletionPolicy: "Retain",
    UpdateReplacePolicy: "Retain",
  });
  template.hasResourceProperties("AWS::ECS::TaskDefinition", {
    ContainerDefinitions: Match.arrayWith([
      Match.objectLike({
        User: "65532:65532",
        ReadonlyRootFilesystem: true,
        Environment: Match.arrayWith([
          { Name: "CBGPU_MULTISCHOOL_V1", Value: "false" },
        ]),
        HealthCheck: Match.objectLike({
          Command: ["CMD", "/cbgpu-healthcheck"],
        }),
      }),
    ]),
  });
  const text = JSON.stringify(template.toJSON());
  assert.ok(!text.includes("AdministratorAccess"));
  assert.ok(!text.includes("s3:DeleteObjectVersion"));
  for (const policy of Object.values(
    template.findResources("AWS::IAM::Policy"),
  )) {
    for (const statement of policy.Properties.PolicyDocument.Statement) {
      if (statement.Effect === "Allow") {
        const actions = Array.isArray(statement.Action)
          ? statement.Action
          : [statement.Action];
        assert.ok(!actions.includes("s3:*"));
        assert.ok(!actions.includes("s3:DeleteObjectVersion"));
      }
    }
  }
  assert.ok(!text.includes("AWS_ACCESS_KEY_ID"));
  assert.ok(!text.includes("AWS_SECRET_ACCESS_KEY"));
  assert.ok(!text.includes("0.0.0.0/0"));
});
