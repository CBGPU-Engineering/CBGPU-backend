import {
  CfnOutput,
  CfnParameter,
  Duration,
  RemovalPolicy,
  Stack,
  type StackProps,
  aws_ec2 as ec2,
  aws_ecs as ecs,
  aws_ecr as ecr,
  aws_iam as iam,
  aws_logs as logs,
  aws_s3 as s3,
  aws_secretsmanager as secrets,
} from "aws-cdk-lib";
import { Construct } from "constructs";
/** Offline-synthesizable foundation: intentionally creates no running service or database. */
export class CloudReadinessStack extends Stack {
  constructor(scope: Construct, id: string, props?: StackProps) {
    super(scope, id, props);
    const vpc = new ec2.Vpc(this, "Network", {
      maxAzs: 2,
      natGateways: 0,
      restrictDefaultSecurityGroup: false,
      ipAddresses: ec2.IpAddresses.cidr("10.77.0.0/16"),
      subnetConfiguration: [
        {
          name: "private",
          subnetType: ec2.SubnetType.PRIVATE_ISOLATED,
          cidrMask: 24,
        },
      ],
    });
    const bucket = new s3.Bucket(this, "StorageObjects", {
      encryption: s3.BucketEncryption.S3_MANAGED,
      versioned: true,
      enforceSSL: true,
      blockPublicAccess: s3.BlockPublicAccess.BLOCK_ALL,
      objectOwnership: s3.ObjectOwnership.BUCKET_OWNER_ENFORCED,
      removalPolicy: RemovalPolicy.RETAIN,
      autoDeleteObjects: false,
    });
    const repository = new ecr.Repository(this, "NativeApiImages", {
      imageTagMutability: ecr.TagMutability.IMMUTABLE,
      imageScanOnPush: true,
      removalPolicy: RemovalPolicy.RETAIN,
      emptyOnDelete: false,
    });
    const cluster = new ecs.Cluster(this, "Applications", { vpc });
    const taskRole = new iam.Role(this, "NativeApiRole", {
      assumedBy: new iam.ServicePrincipal("ecs-tasks.amazonaws.com"),
    });
    const executionRole = new iam.Role(this, "NativeApiExecutionRole", {
      assumedBy: new iam.ServicePrincipal("ecs-tasks.amazonaws.com"),
    });
    repository.grantPull(executionRole);
    const imageDigest = new CfnParameter(this, "NativeApiImageSha256", {
      type: "String",
      allowedPattern: "[a-f0-9]{64}",
      description:
        "Reviewed cbgpu-schools image digest. No image is built/pushed by synth.",
    });
    const backend = new CfnParameter(this, "PrivateBackendOrigin", {
      type: "String",
      allowedPattern: "https://[a-zA-Z0-9.-]+(:[0-9]{1,5})?",
      description:
        "Private TLS Supabase gateway; restore/role validation is a prerequisite. Not an RDS promise.",
    });
    const secretArn = new CfnParameter(this, "ExistingBackendSecretArn", {
      type: "String",
      noEcho: true,
      allowedPattern:
        "arn:[a-z-]+:secretsmanager:[a-z0-9-]+:[0-9]{12}:secret:[a-zA-Z0-9/_+=.@-]+",
      description:
        "Existing Secrets Manager JSON with anon_key. No secret values/defaults or credentials in synthesis.",
    });
    const backendSecret = secrets.Secret.fromSecretCompleteArn(
      this,
      "BackendSecret",
      secretArn.valueAsString,
    );
    backendSecret.grantRead(executionRole);
    const logGroup = new logs.LogGroup(this, "NativeApiLogs", {
      retention: logs.RetentionDays.ONE_MONTH,
      removalPolicy: RemovalPolicy.RETAIN,
    });
    logGroup.grantWrite(executionRole);
    const task = new ecs.FargateTaskDefinition(this, "SchoolApiCandidate", {
      cpu: 512,
      memoryLimitMiB: 1024,
      taskRole,
      executionRole,
      runtimePlatform: {
        cpuArchitecture: ecs.CpuArchitecture.X86_64,
        operatingSystemFamily: ecs.OperatingSystemFamily.LINUX,
      },
    });
    const linuxParameters = new ecs.LinuxParameters(this, "NativeApiLinux", {
      initProcessEnabled: true,
    });
    linuxParameters.dropCapabilities(ecs.Capability.ALL);
    task.addContainer("school-api", {
      image: ecs.ContainerImage.fromRegistry(
        `${repository.repositoryUri}@sha256:${imageDigest.valueAsString}`,
      ),
      user: "65532:65532",
      readonlyRootFilesystem: true,
      stopTimeout: Duration.seconds(45),
      logging: ecs.LogDrivers.awsLogs({ logGroup, streamPrefix: "schools" }),
      environment: {
        CBGPU_MULTISCHOOL_V1: "false",
        CBGPU_SCHOOLS_LISTEN: "127.0.0.1:3546",
        CBGPU_SCHOOLS_BACKEND_URL: backend.valueAsString,
        CBGPU_HEALTH_PORT: "3546",
      },
      secrets: {
        CBGPU_SCHOOLS_ANON_KEY: ecs.Secret.fromSecretsManager(
          backendSecret,
          "anon_key",
        ),
      },
      healthCheck: {
        command: ["CMD", "/cbgpu-healthcheck"],
        interval: Duration.seconds(30),
        timeout: Duration.seconds(5),
        retries: 3,
        startPeriod: Duration.seconds(20),
      },
      linuxParameters,
    });
    // Only the actual Supabase Storage API task may receive object-bucket permissions.
    const storageRole = new iam.Role(this, "StorageApiRole", {
      assumedBy: new iam.ServicePrincipal("ecs-tasks.amazonaws.com"),
    });
    storageRole.addToPolicy(
      new iam.PolicyStatement({
        actions: ["s3:ListBucket", "s3:GetBucketLocation"],
        resources: [bucket.bucketArn],
      }),
    );
    storageRole.addToPolicy(
      new iam.PolicyStatement({
        actions: [
          "s3:GetObject",
          "s3:GetObjectVersion",
          "s3:PutObject",
          "s3:DeleteObject",
          "s3:AbortMultipartUpload",
          "s3:ListMultipartUploadParts",
        ],
        resources: [bucket.arnForObjects("*")],
      }),
    );
    vpc.addGatewayEndpoint("S3PrivateEndpoint", {
      service: ec2.GatewayVpcEndpointAwsService.S3,
    });
    new CfnOutput(this, "NativeImages", { value: repository.repositoryUri });
    new CfnOutput(this, "StorageBucket", { value: bucket.bucketName });
    new CfnOutput(this, "ReviewedTaskDefinition", {
      value: task.taskDefinitionArn,
    });
    new CfnOutput(this, "StorageApiRoleArn", { value: storageRole.roleArn });
    new CfnOutput(this, "PrivateCluster", { value: cluster.clusterName });
    this.templateOptions.description =
      "CBGPU offline readiness foundation. No ECS service, public ingress, database, school GPU, DNS change or data move. Restore roles/storage validation required before activation.";
  }
}
