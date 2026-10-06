import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  // Static export: Caddy serves out/ and proxies /api to the backend on the same origin.
  output: "export",
  trailingSlash: true,
};

export default nextConfig;
