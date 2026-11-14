package termtrace

import (
	"context"
	"encoding/json/jsontext"
)

// Capabilities probes the installation, dependencies, and media capabilities. Canceling ctx
// stops the probes.
func (c *Client) Capabilities(ctx context.Context) (CapabilityReport, error) {
	return c.systemCapabilities(ctx, systemParams{})
}

// Versions reports the build and protocol versions of the CLI.
func (c *Client) Versions(ctx context.Context) (Versions, error) {
	return c.systemVersions(ctx, systemParams{})
}

// Schemas lists the names of the public JSON Schemas.
func (c *Client) Schemas(ctx context.Context) ([]string, error) {
	return c.systemSchemas(ctx, systemParams{})
}

// Schema returns one public JSON Schema by name.
func (c *Client) Schema(ctx context.Context, name string) (jsontext.Value, error) {
	return c.systemSchema(ctx, schemaParams{Name: name})
}
