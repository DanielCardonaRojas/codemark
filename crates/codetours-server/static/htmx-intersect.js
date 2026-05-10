(function () {
    htmx.defineExtension('intersect', {
        onEvent: function (name, evt) {
            if (name === "htmx:afterProcessNode") {
                var element = evt.detail.elt;
                var attributeName = "hx-intersect";
                if (element.hasAttribute(attributeName) || element.hasAttribute("data-" + attributeName)) {
                    var callback = function (entries, observer) {
                        entries.forEach(function (entry) {
                            if (entry.isIntersecting) {
                                htmx.trigger(element, "intersect", {});
                                if (element.getAttribute("hx-intersect-once") === "true") {
                                    observer.unobserve(element);
                                }
                            }
                        });
                    };

                    var options = {
                        root: null,
                        rootMargin: element.getAttribute("hx-intersect-margin") || '0px',
                        threshold: parseFloat(element.getAttribute("hx-intersect-threshold")) || 0.0
                    };

                    var observer = new IntersectionObserver(callback, options);
                    observer.observe(element);
                }
            }
        }
    });
})();
