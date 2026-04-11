# Deploying Savants to AWS + GCP

## Overview

This guide deploys Savants to monitor your AWS and GCP environments
from your existing astra homelab. The architecture:

```
astra (homelab)
├── savants CLI (already running)
├── savants graph engine (already running)
├── K8s watcher → astra-k3s cluster (already connected)
│
├── AWS agent → monitors your AWS environment
│   ├── EKS clusters (pods, deployments, logs)
│   ├── EC2 instances (host monitoring via SSM)
│   ├── RDS databases (health, connections)
│   └── CloudWatch (alarms, metrics)
│
└── GCP agent → monitors your GCP environment
    ├── GKE clusters (pods, deployments, logs)
    ├── Compute Engine (host monitoring)
    ├── Cloud SQL (health)
    └── Cloud Monitoring (alerts)
```

All data flows INTO astra. Nothing is deployed in the cloud except
read-only IAM roles. Your graph stays local. No data leaves your machine.

---

## Step 0: Install Cloud CLIs

Already added to nix-config. Apply:

```bash
cd ~/git/bernadinm/nix-config
sudo nixos-rebuild switch --flake .#astra
```

Verify:
```bash
aws --version
gcloud --version
```

---

## Step 1: AWS Setup

### 1a. Configure AWS credentials

```bash
aws configure
# AWS Access Key ID: (from IAM console)
# AWS Secret Access Key: (from IAM console)
# Default region: us-east-1
# Default output format: json
```

Or use SSO if your org uses it:
```bash
aws configure sso
```

### 1b. Create a read-only IAM policy for Savants

Savants only needs READ access. Never write. Create this policy:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "SavantsReadOnly",
      "Effect": "Allow",
      "Action": [
        "eks:DescribeCluster",
        "eks:ListClusters",
        "eks:ListNodegroups",
        "ec2:DescribeInstances",
        "ec2:DescribeVolumes",
        "ec2:DescribeSecurityGroups",
        "rds:DescribeDBInstances",
        "rds:DescribeDBClusters",
        "elasticloadbalancing:DescribeLoadBalancers",
        "elasticloadbalancing:DescribeTargetGroups",
        "lambda:ListFunctions",
        "lambda:GetFunction",
        "ecs:ListClusters",
        "ecs:DescribeServices",
        "ecs:ListTasks",
        "cloudwatch:DescribeAlarms",
        "cloudwatch:GetMetricData",
        "logs:DescribeLogGroups",
        "logs:GetLogEvents",
        "iam:ListRoles",
        "iam:ListUsers",
        "s3:ListAllMyBuckets",
        "sts:GetCallerIdentity"
      ],
      "Resource": "*"
    }
  ]
}
```

Create the policy:
```bash
aws iam create-policy \
  --policy-name SavantsReadOnly \
  --policy-document file://savants-aws-policy.json
```

Create a dedicated IAM user:
```bash
aws iam create-user --user-name savants-reader
aws iam attach-user-policy \
  --user-name savants-reader \
  --policy-arn arn:aws:iam::YOUR_ACCOUNT:policy/SavantsReadOnly
aws iam create-access-key --user-name savants-reader
# Save the access key and secret key
```

### 1c. Connect to EKS clusters

```bash
# List your EKS clusters
aws eks list-clusters --region us-east-1

# Add each cluster to your kubeconfig
aws eks update-kubeconfig --name YOUR-CLUSTER --region us-east-1

# Verify
kubectl config get-contexts
kubectl --context YOUR-CLUSTER get pods -A | head -5
```

### 1d. Run Savants against AWS

```bash
# Snapshot the EKS cluster
savants k8s snapshot YOUR-CLUSTER --context YOUR-CLUSTER

# Or watch it live
savants k8s watch YOUR-CLUSTER --context YOUR-CLUSTER --logs --tail-lines 500

# Run host snapshot (monitors astra itself)
savants host snapshot

# Full diagnosis
savants story
```

---

## Step 2: GCP Setup

### 2a. Configure GCP credentials

```bash
gcloud auth login
gcloud config set project YOUR-PROJECT-ID
```

### 2b. Create a read-only service account

```bash
# Create service account
gcloud iam service-accounts create savants-reader \
  --display-name "Savants Read-Only"

# Grant minimal roles (viewer only)
gcloud projects add-iam-policy-binding YOUR-PROJECT \
  --member "serviceAccount:savants-reader@YOUR-PROJECT.iam.gserviceaccount.com" \
  --role "roles/viewer"

gcloud projects add-iam-policy-binding YOUR-PROJECT \
  --member "serviceAccount:savants-reader@YOUR-PROJECT.iam.gserviceaccount.com" \
  --role "roles/container.viewer"

# Create and download key
gcloud iam service-accounts keys create ~/.savants/gcp-key.json \
  --iam-account savants-reader@YOUR-PROJECT.iam.gserviceaccount.com

export GOOGLE_APPLICATION_CREDENTIALS=~/.savants/gcp-key.json
```

### 2c. Connect to GKE clusters

```bash
# List GKE clusters
gcloud container clusters list

# Get credentials for each cluster
gcloud container clusters get-credentials YOUR-CLUSTER \
  --region us-central1 --project YOUR-PROJECT

# Verify
kubectl config get-contexts
kubectl --context gke_YOUR-PROJECT_us-central1_YOUR-CLUSTER get pods -A | head -5
```

### 2d. Run Savants against GCP

```bash
# Same commands — Savants doesn't care if it's EKS, GKE, or k3s
savants k8s snapshot YOUR-GKE-CLUSTER
savants k8s watch YOUR-GKE-CLUSTER --logs
savants story
```

---

## Step 3: Deploy savants.cloud (your private instance)

For federation across all environments:

### 3a. Create a GCP project for savants.cloud

```bash
gcloud projects create savants-cloud-prod
gcloud config set project savants-cloud-prod

# Enable required APIs
gcloud services enable \
  run.googleapis.com \
  sqladmin.googleapis.com \
  redis.googleapis.com \
  secretmanager.googleapis.com
```

### 3b. Create Cloud SQL (Postgres)

```bash
gcloud sql instances create savants-db \
  --database-version POSTGRES_16 \
  --cpu 1 --memory 4GB \
  --region us-central1 \
  --root-password YOUR-DB-PASSWORD

gcloud sql databases create savants --instance savants-db
```

### 3c. Build and deploy the API server

```bash
cd savants-cloud

# Build container
gcloud builds submit --tag gcr.io/savants-cloud-prod/savants-cloud:latest

# Deploy to Cloud Run
gcloud run deploy savants-cloud \
  --image gcr.io/savants-cloud-prod/savants-cloud:latest \
  --platform managed \
  --region us-central1 \
  --set-env-vars "DATABASE_URL=postgres://postgres:YOUR-DB-PASSWORD@/savants?host=/cloudsql/savants-cloud-prod:us-central1:savants-db" \
  --set-env-vars "JWT_SECRET=$(openssl rand -hex 32)" \
  --add-cloudsql-instances savants-cloud-prod:us-central1:savants-db \
  --allow-unauthenticated \
  --min-instances 0 \
  --max-instances 3 \
  --memory 512Mi
```

### 3d. Point your domain

```bash
# Get the Cloud Run URL
gcloud run services describe savants-cloud --region us-central1 --format 'value(status.url)'

# In Cloudflare DNS: CNAME api.savants.cloud → the Cloud Run URL
```

### 3e. Connect your CLI

```bash
savants connect
# Opens browser → sign in → done
# Now savants story shows data from ALL environments
```

---

## Step 4: Verify everything works

```bash
# List all connected contexts
kubectl config get-contexts

# Run savants up — should find all clusters
savants up

# Full cross-environment diagnosis
savants story --since-minutes 0

# Generate a report
savants report > ~/infrastructure-report.md
```

---

## Security checklist

- [ ] AWS IAM policy is READ-ONLY (no write permissions)
- [ ] GCP service account is roles/viewer only
- [ ] No secrets in the Savants graph (verified via secret scrubber)
- [ ] savants.cloud uses HTTPS only (Cloud Run enforces this)
- [ ] JWT secret stored in GCP Secret Manager (not env var in production)
- [ ] savants-reader IAM user has no console access
- [ ] All kubeconfig contexts use short-lived tokens (not long-lived certs)

---

## AWS Marketplace (future)

When ready for public availability:

1. Create an AWS Marketplace seller account
2. Package savants-cloud as a container product
3. List as SaaS with metered billing (resources under management)
4. Customers deploy with one click, billing through their AWS account
5. No procurement process needed — uses committed AWS spend

This is Phase 5 in the roadmap. Not needed for your private deployment.
