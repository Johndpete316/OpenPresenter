//! What the unofficial spec documents. Anything outside these lists is still
//! parsed and preserved, but reported as "unknown" by `pp6tool report`.

/// Elements described by the spec.
pub const KNOWN_ELEMENTS: &[&str] = &[
    "RVPresentationDocument",
    "RVTimeline",
    "array",
    "RVSlideGrouping",
    "RVDisplaySlide",
    "RVTextElement",
    "RVRect3D",
    "shadow",
    "dictionary",
    "NSColor",
    "NSNumber",
    "NSString",
];

pub fn is_known_element(name: &str) -> bool {
    KNOWN_ELEMENTS.contains(&name)
}

/// Attributes the spec lists for `element`, or `None` for an unknown element.
pub fn known_attrs(element: &str) -> Option<&'static [&'static str]> {
    Some(match element {
        "RVPresentationDocument" => &[
            "height",
            "width",
            "docType",
            "versionNumber",
            "usedCount",
            "backgroundColor",
            "drawingBackgroundColor",
            "CCLIDisplay",
            "lastDateUsed",
            "selectedArrangementID",
            "category",
            "resourcesDirectory",
            "notes",
            "CCLISongTitle",
            "chordChartPath",
            "os",
            "buildNumber",
        ],
        "RVTimeline" => &[
            "timeOffset",
            "duration",
            "selectedMediaTrackIndex",
            "loop",
            "rvXMLIvarName",
        ],
        "RVSlideGrouping" => &["name", "color", "uuid"],
        "RVDisplaySlide" => &[
            "backgroundColor",
            "highlightColor",
            "drawingBackgroundColor",
            "enabled",
            "hotKey",
            "label",
            "notes",
            "UUID",
            "chordChartPath",
        ],
        "RVTextElement" => &[
            "displayName",
            "UUID",
            "typeID",
            "displayDelay",
            "locked",
            "persistent",
            "fromTemplate",
            "opacity",
            "source",
            "bezelRadius",
            "rotation",
            "drawingFill",
            "drawingShadow",
            "drawingStroke",
            "fillColor",
            "adjustsHeightToFit",
            "verticalAlignment",
            "revealType",
        ],
        "array" | "RVRect3D" | "shadow" | "dictionary" | "NSString" => &["rvXMLIvarName"],
        "NSColor" | "NSNumber" => &["rvXMLDictionaryKey"],
        _ => return None,
    })
}
