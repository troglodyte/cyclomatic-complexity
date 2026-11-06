// Fixture for checking editor diagnostics by hand — the JavaScript counterpart of
// test.ts. With the default threshold of 11: nothing under 11, warnings from 11 to
// 21, errors from 22 up.
//
// A `.js` file is lexed with JSX enabled, so this also covers the JSX path: the
// `&&` inside element text below is text, not an operator, and must not count.

/** Scores 2 — under the threshold, so no diagnostic. */
export function slugify(value) {
  return value ? value.toLowerCase().replace(/[^a-z0-9]+/g, "-") : "";
}

/** Regex versus division, and a template literal with a nested substitution. */
export function describeRatio(count, total) {
  const ratio = total / count / 2;
  return `${count} of ${total} (${ratio > 0.5 ? "most" : "some"})`;
}

/** Scores in the teens — expect a warning. */
export function normalizeConfig(input) {
  const config = { ...input };

  if (!config.name) {
    config.name = "unnamed";
  }
  if (config.retries == null || config.retries < 0) {
    config.retries = 3;
  }
  if (!Array.isArray(config.hosts)) {
    config.hosts = [];
  }

  for (const host of config.hosts) {
    if (!host.port) {
      host.port = 443;
    }
    if (host.tls === undefined) {
      host.tls = host.port === 443;
    }
    if (host.weight && host.weight > 100) {
      host.weight = 100;
    }
  }

  config.timeout ||= 30_000;
  config.label ??= config.name;

  return config;
}

/** Well past double the threshold — expect an error. */
export function reduceEvents(events, options) {
  let opened = 0;
  let closed = 0;
  let errors = 0;

  for (const event of events) {
    if (!event || !event.type) {
      continue;
    }

    switch (event.type) {
      case "open":
        opened += 1;
        break;
      case "close":
        closed += 1;
        break;
      case "error":
        errors += 1;
        break;
      case "retry":
        if (event.attempt > 3) {
          errors += 1;
        }
        break;
      default:
        break;
    }

    if (event.durationMs && event.durationMs > 1000) {
      if (event.type === "open" || event.type === "retry") {
        errors += 1;
      }
    }

    if (event.tags && event.tags.length > 0) {
      for (const tag of event.tags) {
        if (tag === "critical") {
          errors += 1;
        }
      }
    }

    while (event.pending && event.pending.length > 0) {
      event.pending.pop();
    }
  }

  try {
    if (opened < closed) {
      throw new Error("unbalanced");
    }
  } catch (err) {
    if (options?.strict) {
      throw err;
    }
    errors += 1;
  }

  const rate = opened > 0 ? errors / opened : 0;
  return { opened, closed, errors, healthy: rate < 0.1 && errors < 5 };
}

/** JSX: the `&&` in element text is text. Nested callbacks score on their own. */
export function EventList({ events, loading, emptyLabel }) {
  if (loading) {
    return <Spinner />;
  }
  if (!events || events.length === 0) {
    return <p>{emptyLabel ?? "Nothing yet"}</p>;
  }

  return (
    <ul className={events.length > 10 ? "list list--dense" : "list"}>
      <li className="hint">open && close events are paired — a && b is text here</li>
      {events.map((event) => (
        <li key={event.id} data-kind={event.type}>
          {event.type === "error" ? <strong>{event.message}</strong> : event.message}
          {event.retryable && <button onClick={() => retry(event)}>Retry</button>}
        </li>
      ))}
    </ul>
  );
}
