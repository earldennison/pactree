use std::fmt;
use std::io::Read;

use serde::de::{self, DeserializeSeed, IgnoredAny, MapAccess, Visitor};

use crate::error::{PactError, PactResult};
use crate::store::PactRef;

pub(crate) trait MembershipSink {
    fn first_seen(&mut self, component_id: &str, revision: u64) -> PactResult<()>;
    fn lifecycle_seen(&mut self, component_id: &str, tick: u64) -> PactResult<()>;
}

#[derive(Debug, Default)]
pub(crate) struct BranchScalars {
    pub(crate) found: bool,
    pub(crate) head_revision: Option<u64>,
    pub(crate) lifecycle_tick: u64,
    pub(crate) base_ref: Option<PactRef>,
    pub(crate) has_parent_branch: bool,
    pub(crate) has_fork_ref: bool,
    pub(crate) derived_depth: bool,

    pub(crate) ledger_parent: Option<serde_json::Value>,

    pub(crate) root: Option<serde_json::Map<String, serde_json::Value>>,
}

struct Cx<'a> {
    branch_id: &'a str,
    sink: &'a mut dyn MembershipSink,
    out: BranchScalars,
    error: Option<PactError>,
}

impl Cx<'_> {
    fn note<E: de::Error>(&mut self, result: PactResult<()>) -> Result<(), E> {
        match result {
            Ok(()) => Ok(()),
            Err(error) => {
                let message = error.to_string();
                self.error = Some(error);
                Err(E::custom(message))
            }
        }
    }
}

pub(crate) fn stream_branch_membership(
    reader: impl Read,
    in_sections: bool,
    branch_id: &str,
    sink: &mut dyn MembershipSink,
) -> PactResult<BranchScalars> {
    let mut cx = Cx {
        branch_id,
        sink,
        out: BranchScalars::default(),
        error: None,
    };
    let mut deserializer = serde_json::Deserializer::from_reader(std::io::BufReader::new(reader));
    let parsed = if in_sections {
        Sections { cx: &mut cx }.deserialize(&mut deserializer)
    } else {
        Checkpoint { cx: &mut cx }.deserialize(&mut deserializer)
    };
    if let Some(error) = cx.error.take() {
        return Err(error);
    }
    parsed.map_err(|error| {
        PactError::corrupt(format!("invalid Pact baseline checkpoint: {error}"))
    })?;
    if !cx.out.found {
        return Err(PactError::corrupt(
            "Pact baseline has no entry for the branch",
        ));
    }
    Ok(cx.out)
}

macro_rules! map_seed {
    ($name:ident, $expecting:literal, |$cx:ident, $key:ident, $map:ident| $body:block) => {
        struct $name<'c, 'a> {
            cx: &'c mut Cx<'a>,
        }

        impl<'de> DeserializeSeed<'de> for $name<'_, '_> {
            type Value = ();

            fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
            where
                D: de::Deserializer<'de>,
            {
                deserializer.deserialize_any(self)
            }
        }

        impl<'de> Visitor<'de> for $name<'_, '_> {
            type Value = ();

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str($expecting)
            }

            fn visit_unit<E: de::Error>(self) -> Result<(), E> {
                Ok(())
            }

            fn visit_none<E: de::Error>(self) -> Result<(), E> {
                Ok(())
            }

            fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
                while seq.next_element::<IgnoredAny>()?.is_some() {}
                Ok(())
            }

            fn visit_map<A: MapAccess<'de>>(self, mut $map: A) -> Result<(), A::Error> {
                let $cx = self.cx;
                while let Some($key) = $map.next_key::<String>()? {
                    $body
                }
                Ok(())
            }
        }
    };
}

map_seed!(
    Sections,
    "a compact transaction's sections object",
    |cx, key, map| {
        if key == "head_checkpoint" {
            map.next_value_seed(Checkpoint { cx })?;
        } else {
            map.next_value::<IgnoredAny>()?;
        }
    }
);

map_seed!(Checkpoint, "a Pact head checkpoint", |cx, key, map| {
    match key.as_str() {
        "ledger" => map.next_value_seed(Checkpoint { cx })?,
        "branches" => map.next_value_seed(Branches { cx })?,
        "parent" => {
            let parent = map.next_value::<serde_json::Value>()?;
            if !parent.is_null() {
                cx.out.ledger_parent = Some(parent);
            }
        }
        _ => {
            map.next_value::<IgnoredAny>()?;
        }
    }
});

map_seed!(Branches, "the checkpoint's branches", |cx, key, map| {
    if key == cx.branch_id {
        cx.out.found = true;
        map.next_value_seed(Branch { cx })?;
    } else {
        map.next_value::<IgnoredAny>()?;
    }
});

map_seed!(Branch, "one branch of the checkpoint", |cx, key, map| {
    match key.as_str() {
        "head_revision" => cx.out.head_revision = Some(map.next_value::<u64>()?),
        "lifecycle_tick" => cx.out.lifecycle_tick = map.next_value::<u64>()?,
        "base_ref" => cx.out.base_ref = Some(map.next_value::<PactRef>()?),
        "parent_branch_id" => {
            cx.out.has_parent_branch = !map.next_value::<serde_json::Value>()?.is_null();
        }
        "fork_ref" => {
            cx.out.has_fork_ref = !map.next_value::<serde_json::Value>()?.is_null();
        }
        "aux" => {
            let aux = map.next_value::<serde_json::Map<String, serde_json::Value>>()?;
            cx.out.derived_depth = aux
                .get(crate::store::PACT_DERIVED_DEPTH_AUX_KEY)
                .and_then(serde_json::Value::as_bool)
                == Some(true);
        }
        "current" => map.next_value_seed(Current { cx })?,
        _ => {
            map.next_value::<IgnoredAny>()?;
        }
    }
});

map_seed!(Current, "the branch's current view", |cx, key, map| {
    match key.as_str() {
        "tree" => map.next_value_seed(Tree { cx })?,
        "lifecycle_seen" => map.next_value_seed(LifecycleSeen { cx })?,
        _ => {
            map.next_value::<IgnoredAny>()?;
        }
    }
});

map_seed!(Tree, "the branch's tree", |cx, key, map| {
    match key.as_str() {
        "first_seen" => map.next_value_seed(FirstSeen { cx })?,
        "root" => map.next_value_seed(RootShell { cx })?,
        _ => {
            map.next_value::<IgnoredAny>()?;
        }
    }
});

struct RootShell<'c, 'a> {
    cx: &'c mut Cx<'a>,
}

impl<'de> DeserializeSeed<'de> for RootShell<'_, '_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for RootShell<'_, '_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("the branch's root component")
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: de::MapAccess<'de>,
    {
        let mut shell = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if key == "children" {
                map.next_value::<IgnoredAny>()?;
            } else {
                let value = map.next_value::<serde_json::Value>()?;
                shell.insert(key, value);
            }
        }
        self.cx.out.root = Some(shell);
        Ok(())
    }
}

map_seed!(FirstSeen, "the first_seen map", |cx, key, map| {
    let revision = map.next_value::<u64>()?;
    let result = cx.sink.first_seen(&key, revision);
    cx.note(result)?;
});

map_seed!(LifecycleSeen, "the lifecycle_seen map", |cx, key, map| {
    let tick = map.next_value::<u64>()?;
    let result = cx.sink.lifecycle_seen(&key, tick);
    cx.note(result)?;
});

#[cfg(test)]
mod tests;
