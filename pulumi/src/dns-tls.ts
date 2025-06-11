import * as awsclassic from "@pulumi/aws";
import * as pulumi from "@pulumi/pulumi";
import * as cfProvider from "@pulumi/cloudflare";

const cloudflareApiToken = pulumi.secret(
  awsclassic.ssm
    .getParameter({
      name: "/lambda-rssfilter/cloudflare-token",
      withDecryption: true,
    })
    .then((result) => result.value),
);

const cloudflareProvider = new cfProvider.Provider("cloudflare", {
  apiToken: cloudflareApiToken,
});

export function cloudflare(subdomain: string, zone: string) {
  const zoneId = cfProvider
    .getZoneOutput(
      {
        filter: {
          match: "all",
          name: zone,
        },
      },
      { provider: cloudflareProvider },
    )
    .apply(({ zoneId }) => {
      if (zoneId === undefined) {
        throw new Error(
          `Zone ${zone} not found in Cloudflare. This must be created externally.`,
        );
      }

      return zoneId;
    });

  const zoneSettings = {
    always_use_https: "on",
    automatic_https_rewrites: "on",
    browser_cache_ttl: 14400,
    development_mode: "off",
    http3: "on",
    ip_geolocation: "on",
    ipv6: "on",
    min_tls_version: "1.2",
    opportunistic_encryption: "on",
    opportunistic_onion: "on",
    security_level: "medium",
    server_side_exclude: "on",
    ssl: "strict",
    tls_1_3: "on",
  };

  // Create a ZoneSetting for each settingId/value pair
  for (const [settingId, value] of Object.entries(zoneSettings)) {
    const setting = new cfProvider.ZoneSetting(
      settingId,
      {
        zoneId,
        settingId,
        value,
      },
      {
        provider: cloudflareProvider,
      },
    );
  }
}
