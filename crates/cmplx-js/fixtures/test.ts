// Fixture for checking editor diagnostics by hand. `cargo test` covers scoring;
// this file exists so you can open it in Zed or a JetBrains IDE and see all three
// states at once. With the default threshold of 11: nothing under 11, warnings
// from 11 to 21, errors from 22 up.
//
// A `.ts` file also exercises the TypeScript path, where JSX lexing is off and
// type positions are masked — the conditional type and the function-type
// annotations below must not be counted as branches.

type Money = { amount: number; currency: string };

type Unwrap<T> = T extends Promise<infer U> ? U : T;

interface Order {
  id: string;
  items: Array<{ sku: string; qty: number; price: number }>;
  coupon?: string;
  tier?: "free" | "pro" | "enterprise";
  shippingCountry?: string;
  expedited?: boolean;
  giftWrap?: boolean;
}

/** Scores 2 — under the threshold, so this should show no diagnostic at all. */
export function formatMoney(m: Money): string {
  return m.currency === "USD" ? `$${m.amount.toFixed(2)}` : `${m.amount} ${m.currency}`;
}

/** A function-type annotation, not an arrow function. Should not be reported. */
export const onSettled: (order: Order, err?: Error) => void = (order) => {
  console.log(order.id);
};

/** Scores in the teens — expect a warning. */
export function validateOrder(order: Order): string[] {
  const errors: string[] = [];

  if (!order.id) {
    errors.push("missing id");
  }
  if (!order.items || order.items.length === 0) {
    errors.push("no items");
  }

  for (const item of order.items ?? []) {
    if (!item.sku) {
      errors.push("item missing sku");
    }
    if (item.qty <= 0) {
      errors.push("quantity must be positive");
    }
    if (item.price < 0) {
      errors.push("price cannot be negative");
    }
    if (item.qty > 100 && order.tier !== "enterprise") {
      errors.push("quantity over limit");
    }
  }

  if (order.coupon && order.coupon.length < 3) {
    errors.push("coupon too short");
  }
  while (errors.length > 10) {
    errors.pop();
  }

  return errors;
}

/** Well past double the threshold — expect an error. */
export function applyPricingRules(order: Order): Money {
  let total = 0;

  for (const item of order.items) {
    total += item.price * item.qty;
    if (item.qty >= 10) {
      total -= item.price;
    }
    if (item.price > 1000 || item.sku.startsWith("LUX")) {
      total += 25;
    }
  }

  switch (order.tier) {
    case "free":
      break;
    case "pro":
      total *= 0.9;
      break;
    case "enterprise":
      total *= 0.8;
      break;
    default:
      break;
  }

  if (order.coupon === "WELCOME") {
    total -= 5;
  } else if (order.coupon === "BIGSPEND") {
    if (total > 500) {
      total -= 100;
    } else if (total > 200) {
      total -= 25;
    }
  }

  if (order.shippingCountry && order.shippingCountry !== "US") {
    total += 15;
  }
  if (order.expedited) {
    total += order.tier === "enterprise" ? 0 : 20;
  }
  if (order.giftWrap ?? false) {
    total += 4;
  }
  if (order.items.length > 25) {
    total -= 10;
  }
  if (order.coupon?.startsWith("VIP") || order.tier === "pro") {
    total -= 2;
  }
  if (total > 10_000) {
    total *= 0.95;
  }

  try {
    if (total < 0) {
      throw new RangeError("negative total");
    }
  } catch {
    total = 0;
  }

  const rounded = total > 0 ? Math.round(total * 100) / 100 : 0;
  return { amount: rounded, currency: "USD" };
}

export class OrderRouter {
  /** Also over double the threshold — expect an error on the method line. */
  route(order: Order, region?: string): string {
    if (!order.id) {
      return "reject";
    }

    if (region === "eu") {
      if (order.tier === "enterprise") {
        return "eu-priority";
      }
      if (order.expedited) {
        return "eu-fast";
      }
      return "eu-standard";
    } else if (region === "apac") {
      if (order.items.length > 20 || order.expedited) {
        return "apac-fast";
      }
      return "apac-standard";
    } else if (region === "us" || region === undefined) {
      if (order.tier === "free" && order.items.length > 50) {
        return "us-batch";
      }
      if (order.giftWrap) {
        return "us-giftwrap";
      }
      if (order.shippingCountry && order.shippingCountry !== "US") {
        return "us-export";
      }
      return "us-standard";
    } else if (region === "latam") {
      if (order.expedited && order.tier !== "free") {
        return "latam-fast";
      }
      if (order.giftWrap || order.items.length > 5) {
        return "latam-gift";
      }
      return "latam-standard";
    }

    switch (order.tier) {
      case "pro":
        return "row-pro";
      case "enterprise":
        return "row-enterprise";
      default:
        return order.expedited ? "row-fast" : "row-standard";
    }
  }

  /** Nested functions are scored on their own, so this arrow gets its own row. */
  summarize(orders: Order[]): number {
    return orders.reduce((sum, order) => {
      if (!order.items) {
        return sum;
      }
      const line = order.items.length > 0 ? order.items.length : 0;
      return sum + (order.expedited ? line * 2 : line);
    }, 0);
  }
}
