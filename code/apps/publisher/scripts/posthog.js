// Standard asynchronous PostHog loader; configuration comes from Drive site.json.
!function (t, e) {
  var o, n, p, r;
  e.__SV || (window.posthog = e,
    e._i = [],
    e.init = function (i, s, a) {
      function g(t, e) {
        var o = e.split(".");
        2 == o.length && (t = t[o[0]], e = o[1]),
          t[e] = function () {
            t.push([e].concat(Array.prototype.slice.call(arguments, 0)));
          };
      }
      (p = t.createElement("script")).type = "text/javascript",
        p.crossOrigin = "anonymous",
        p.async = !0,
        p.src = s.api_host + "/static/array.js",
        (r = t.getElementsByTagName("script")[0]).parentNode.insertBefore(p, r);
      var u = e;
      for (
        void 0 !== a ? u = e[a] = [] : a = "posthog",
          u.people = u.people || [],
          o =
            "init capture register register_once register_for_session unregister unregister_for_session identify setPersonProperties group reset get_distinct_id get_session_id alias set_config startSessionRecording stopSessionRecording opt_in_capturing opt_out_capturing debug"
              .split(" "),
          n = 0;
        n < o.length;
        n++
      ) g(u, o[n]);
      e._i.push([i, s, a]);
    },
    e.__SV = 1);
}(document, window.posthog || []);
posthog.init(__PROJECT_TOKEN__, {
  api_host: __API_HOST__,
  defaults: "2026-08-30",
  autocapture: true,
  capture_pageview: true,
  disable_session_recording: false,
  session_recording: { maskAllInputs: true },
  person_profiles: "identified_only",
  before_send: function (event) {
    if (event) {
      event.properties.site_environment = location.hostname === "randallb.com"
        ? "production"
        : "preview";
    }
    return event;
  },
});
