const PRIMARY_HOST = "opskills.app";

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    // Send secondary domains (getopskills.com, opskills.dev) to the primary domain,
    // keeping the path and query. workers.dev and localhost are left alone for testing.
    const isTestHost = url.hostname.endsWith(".workers.dev") || url.hostname === "localhost";
    if (url.hostname !== PRIMARY_HOST && !isTestHost) {
      url.protocol = "https:";
      url.hostname = PRIMARY_HOST;
      url.port = "";
      return Response.redirect(url.toString(), 301);
    }

    return env.ASSETS.fetch(request);
  },
};
