#!/bin/bash
# Deploy savants-cloud from MinIO binaries to k3s
# Run on astra after CI builds the binaries
set -e

echo "=== Deploying savants-cloud ==="

# Download both binaries from MinIO
mc cp astra/savants-releases/cloud/savants-cloud /tmp/savants-cloud
mc cp astra/savants-releases/cloud/savants-cli /tmp/savants-cli
chmod +x /tmp/savants-cloud /tmp/savants-cli

# Build Docker image with BOTH binaries
cat > /tmp/Dockerfile.savants-cloud << 'EOF'
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
COPY savants-cloud /usr/local/bin/savants-cloud
COPY savants-cli /usr/local/bin/savants
EXPOSE 3000
ENV LISTEN_ADDR=0.0.0.0:3000
ENV SAVANTS_HOST=192.168.100.148
ENV SAVANTS_PORT=6379
ENV SAVANTS_MEMORY=savants
CMD ["savants-cloud"]
EOF

docker build -t savants/savants-cloud:latest -f /tmp/Dockerfile.savants-cloud /tmp/

# Import to k3s
docker save savants/savants-cloud:latest | sudo k3s ctr images import -

# Deploy
export KUBECONFIG=/etc/rancher/k3s/k3s.yaml
cd "$(dirname "$0")"
helm upgrade --install savants-cloud deploy/ \
  --namespace savants-cloud \
  --set api.image=savants/savants-cloud:latest \
  --wait --timeout 3m

# Force pod refresh
kubectl rollout restart deployment/savants-cloud-api -n savants-cloud
kubectl rollout status deployment/savants-cloud-api -n savants-cloud --timeout=60s

echo "=== Deployed ==="
kubectl get pods -n savants-cloud
curl -sf https://api.savants.cloud/health && echo " - healthy"
