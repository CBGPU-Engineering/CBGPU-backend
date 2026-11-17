# AWS target candidate

The first compatible target is the existing containerized central service with
the same PostgreSQL/Supabase roles and extensions. RDS is not yet a verified
replacement for the existing privileged role model. Do not change engines as a
side effect of relocation.

`infra/cloud` contains pinned TypeScript CDK source. Offline synthesis defines a
private VPC, retained encrypted/versioned bucket, ECR, cluster and disabled native
API task definition. It creates no running service, database, public ingress or
DNS. No AWS account or resources are needed to run its assertions.

Before a real deployment, decide database hosting, private connectivity, provider
outbound networking, image/log/secret endpoints, TLS ingress and backup storage.
The isolated subnets currently have no NAT: external providers will not work by
assumption. Retain Cloudflare DNS if desired; DNS migration is not a requirement.

Measure actual traffic and price compute, database, storage, egress, load balancer,
NAT and logs for the chosen region before approval. No cost estimate or capacity
claim has been invented. School GPU execution remains outside this central stack.
