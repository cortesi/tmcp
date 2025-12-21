# GitHub MCP Server Example

This example demonstrates how to connect to the GitHub MCP server using OAuth authentication.

## Prerequisites

1. **GitHub OAuth App**: You need to create a GitHub OAuth App:
   - Go to GitHub Settings > Developer settings > OAuth Apps
   - Click "New OAuth App"
   - Set the callback URL to `http://localhost:8080/callback` (or your chosen port)
   - Save the Client ID and Client Secret

## Running the Example

### Using Environment Variables

You can set the credentials as environment variables to avoid typing them:

```bash
export GITHUB_CLIENT_ID="your_client_id"
export GITHUB_CLIENT_SECRET="your_client_secret"
export GITHUB_ACCESS_TOKEN="your_access_token"  # Optional, to skip OAuth flow
```

Then run with:
```bash
# First time (OAuth flow)
cargo run --example ghusers

# With existing token
cargo run --example ghusers -- --access-token $GITHUB_ACCESS_TOKEN
```

### First Time (OAuth Flow)

```bash
cargo run --example ghusers -- \
  --client-id YOUR_CLIENT_ID \
  --client-secret YOUR_CLIENT_SECRET
```

This will:
1. Open your browser for GitHub authorization
2. Start a local server on port 8080 to receive the callback
3. Exchange the authorization code for an access token
4. Connect to the GitHub MCP server
5. List available tools, resources, and prompts

### Subsequent Runs (With Access Token)

After the first run, save the access token and use it directly:

```bash
cargo run --example ghusers -- \
  --client-id YOUR_CLIENT_ID \
  --client-secret YOUR_CLIENT_SECRET \
  --access-token YOUR_ACCESS_TOKEN
```

## Command Line Options

- `--client-id` / `-c`: GitHub OAuth App Client ID (required)
- `--client-secret` / `-s`: GitHub OAuth App Client Secret (required)
- `--port` / `-p`: OAuth callback port (default: 8080)
- `--access-token` / `-t`: Use existing access token (skip OAuth flow)

## GitHub MCP Server Endpoints

The example connects to: `https://api.githubcopilot.com/mcp/x/users/readonly`

Other available endpoints include:
- `/mcp/x/repos/full` - Full repository access
- `/mcp/x/repos/readonly` - Read-only repository access
- `/mcp/x/issues/full` - Full issue access
- `/mcp/x/issues/readonly` - Read-only issue access

## Troubleshooting

1. **"Failed to connect"**: Ensure your OAuth app is properly configured and the callback URL matches
2. **"Unauthorized"**: Your access token may have expired or lack necessary scopes
3. **"CSRF token mismatch"**: Don't use the back button during OAuth flow; start fresh

## Security Notes

- Never commit your Client Secret or Access Token
- Use environment variables or secure credential storage
- GitHub access tokens should be treated as passwords
- Revoke tokens you're no longer using in GitHub Settings