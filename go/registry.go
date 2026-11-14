package termtrace

import (
	"context"
	"time"
)

// Registry is a session registry: aliases, lifecycle, retention, and restart.
type Registry struct {
	client *Client
	root   *Path //exhaustruct:optional
}

// RestartOptions configures a restart.
//
//exhaustruct:optional
type RestartOptions struct {
	// Options sets what the successor may change.
	Options RestartConfiguration
	// Environment replaces the predecessor's environment when set.
	Environment *EnvironmentSpec
	// Timeout bounds successor readiness; the server default when zero.
	Timeout time.Duration
}

// GcOptions configures registry retention.
//
//exhaustruct:optional
type GcOptions struct {
	// OlderThanDays removes completed sessions older than this many days; 30 when nil.
	OlderThanDays *uint64
	// KeepLatest always keeps this many latest sessions; 20 when nil.
	KeepLatest *uint64
	// MaxCompletedBytes caps the bytes of completed sessions when set.
	MaxCompletedBytes *uint64
	// Apply deletes; the call only plans when false.
	Apply bool
}

// Registry returns the registry at root, or the default registry when root is empty.
func (c *Client) Registry(root string) *Registry {
	registry := &Registry{client: c}
	if root != "" {
		registry.root = new(Path(root))
	}

	return registry
}

// List returns registered sessions; includeAll adds sessions that are no longer running.
func (r *Registry) List(ctx context.Context, includeAll bool) ([]RegistryListEntry, error) {
	return r.client.registryList(ctx, listParams{Root: r.root, IncludeAll: includeAll})
}

// Register records session under alias.
func (r *Registry) Register(
	ctx context.Context,
	alias string,
	session string,
) (RegistryEntry, error) {
	return r.client.registryRegister(ctx, registerParams{
		Root:    r.root,
		Alias:   alias,
		Session: Path(session),
	})
}

// Resolve returns the entry of alias, or nil when no alias record exists.
func (r *Registry) Resolve(ctx context.Context, alias string) (*RegistryEntry, error) {
	return r.client.registryResolve(ctx, aliasParams{Root: r.root, Alias: alias})
}

// Lifecycle reports the lifecycle of alias, including pending and incompatible states.
func (r *Registry) Lifecycle(ctx context.Context, alias string) (RegistrySessionResolution, error) {
	return r.client.registryLifecycle(ctx, aliasParams{Root: r.root, Alias: alias})
}

// Remove deletes alias; the session evidence stays.
func (r *Registry) Remove(ctx context.Context, alias string) (RegistryEntry, error) {
	return r.client.registryRemove(ctx, aliasParams{Root: r.root, Alias: alias})
}

// Restart starts a successor of the session behind alias and moves the alias to it once the
// successor is ready.
func (r *Registry) Restart(
	ctx context.Context,
	alias string,
	options RestartOptions,
) (RestartReport, error) {
	return r.client.registryRestart(ctx, restartParams{
		Root:        r.root,
		Alias:       alias,
		Options:     options.Options,
		Environment: options.Environment,
		TimeoutMs:   milliseconds(options.Timeout),
	})
}

// Gc plans registry retention, and applies it when options.Apply is set.
func (r *Registry) Gc(ctx context.Context, options GcOptions) (GarbageCollectionReport, error) {
	return r.client.registryGc(ctx, gcParams{
		Root:              r.root,
		OlderThanDays:     options.OlderThanDays,
		KeepLatest:        options.KeepLatest,
		MaxCompletedBytes: options.MaxCompletedBytes,
		Apply:             options.Apply,
	})
}
