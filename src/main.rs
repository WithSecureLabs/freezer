use clap::Parser;
use k8s_openapi::serde_json;
use kube::{
    api::{Api, DynamicObject, ResourceExt},
    config::Kubeconfig,
    discovery::{verbs, ApiCapabilities, ApiResource, Discovery, Scope},
    Client,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::hash::Hash;

#[derive(Debug, Serialize, Deserialize)]
struct Metadata {
    kube_version: String,
    context_name: String,
    api_versions: Vec<String>,
    preferred_versions: HashMap<String, String>,
    api_resources: Vec<MyApiResource>,
}

#[derive(Debug, Serialize, Deserialize)]
struct MyApiResource {
    name: String,
    namespaced: bool,
    group: String,
    kind: String,
    verbs: Vec<String>,
    preferred: bool,
}

impl MyApiResource {
    fn from_ar_and_caps(ar: ApiResource, caps: ApiCapabilities, preferred: bool) -> Self {
        Self {
            name: ar.plural,
            namespaced: caps.scope == Scope::Namespaced,
            group: ar.api_version,
            kind: ar.kind,
            verbs: caps.operations,
            preferred,
        }
    }
}

fn merge_vecs<T: Eq + Hash>(vec1: Vec<T>, vec2: Vec<T>) -> Vec<T> {
    let mut vec: Vec<_> = Vec::new();
    vec.extend(vec1);
    vec.extend(vec2);
    let set: HashSet<_> = vec.drain(..).collect();
    vec.extend(set.into_iter());
    vec
}

async fn get_version(client: &Client) -> anyhow::Result<String> {
    let url = "/version/".to_string();
    let req = http::Request::get(url).body(Default::default())?;
    let resp = client.request::<serde_json::Value>(req).await?;

    resp.get("gitVersion")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| anyhow::anyhow!("failed to get version from /version endpoint: {:?}", resp))
}

async fn get_context_name() -> anyhow::Result<String> {
    let kubeconfig = Kubeconfig::read();

    match kubeconfig {
        Ok(k) => Ok(k
            .current_context
            .ok_or_else(|| anyhow::anyhow!("failed to get current context"))?),
        Err(_e) => Ok("in-cluster".to_string()),
    }
}

async fn get_namespaces(client: &Client, discovery: &Discovery) -> anyhow::Result<Vec<String>> {
    let mut namespaces: Vec<String> = Vec::new();

    for group in discovery.groups() {
        if group.name() != "" {
            continue;
        }
        for (ar, _) in group.recommended_resources() {
            if ar.kind != "Namespace" {
                continue;
            }
            let api: Api<DynamicObject> = Api::all_with(client.clone(), &ar);

            let list = api.list(&Default::default()).await?;

            namespaces = list.items.iter().map(|i| i.name_any()).collect();
        }
    }

    Ok(namespaces)
}

async fn api_versions(discovery: &Discovery) -> Vec<String> {
    let mut versions: Vec<String> = Vec::new();

    for group in discovery.groups() {
        for ver in group.versions() {
            if group.name() == "" {
                versions.push(format!("{}", ver))
            } else {
                versions.push(format!("{}/{}", group.name(), ver))
            }
        }
    }

    versions.sort();
    versions
}

async fn preferred_api_versions(discovery: &Discovery) -> HashMap<String, String> {
    let mut preferred: HashMap<String, String> = HashMap::new();

    for group in discovery.groups() {
        if group.name() == "" {
            continue;
        }
        preferred.insert(
            group.name().to_string(),
            group.preferred_version_or_latest().to_string(),
        );
    }

    preferred
}

async fn api_resources(discovery: &Discovery) -> Vec<MyApiResource> {
    let mut resources: Vec<MyApiResource> = Vec::new();

    let additional_verbs: HashMap<&str, Vec<String>> = HashMap::from([
        ("roles", vec!["bind".to_string(), "escalate".to_string()]),
        (
            "clusterroles",
            vec!["bind".to_string(), "escalate".to_string()],
        ),
        ("serviceaccounts", vec!["impersonate".to_string()]),
        ("users", vec!["impersonate".to_string()]),
        ("groups", vec!["impersonate".to_string()]),
    ]);

    for group in discovery.groups() {
        let preferred_version = group.preferred_version_or_latest();
        for ver in group.versions() {
            let preferred = ver == preferred_version;
            for (ar, caps) in group.versioned_resources(ver) {
                let subresources = caps.subresources.clone();
                if additional_verbs.contains_key(&ar.plural as &str) {
                    let new_caps = ApiCapabilities {
                        operations: merge_vecs(
                            caps.operations.clone(),
                            additional_verbs.get(&ar.plural as &str).unwrap().to_vec(),
                        ),
                        ..caps.clone()
                    };
                    resources.push(MyApiResource::from_ar_and_caps(
                        ar.clone(),
                        new_caps,
                        preferred,
                    ));
                } else {
                    resources.push(MyApiResource::from_ar_and_caps(ar.clone(), caps, preferred));
                }

                for (sub_ar, sub_caps) in subresources {
                    let new_ar = ApiResource {
                        plural: format!("{}/{}", ar.plural, sub_ar.plural),
                        ..sub_ar
                    };
                    resources.push(MyApiResource::from_ar_and_caps(new_ar, sub_caps, preferred));
                }
            }
        }
    }

    if resources.iter().all(|x| x.name != "users") {
        resources.push(MyApiResource {
            name: "users".to_string(),
            namespaced: false,
            group: "".to_string(),
            kind: "User".to_string(),
            verbs: vec!["impersonate".to_string()],
            preferred: true,
        });
    }

    if resources.iter().all(|x| x.name != "groups") {
        resources.push(MyApiResource {
            name: "groups".to_string(),
            namespaced: false,
            group: "".to_string(),
            kind: "Group".to_string(),
            verbs: vec!["impersonate".to_string()],
            preferred: true,
        });
    }

    if resources.iter().all(|x| x.name != "signers") {
        resources.push(MyApiResource {
            name: "signers".to_string(),
            namespaced: false,
            group: "certificates.k8s.io/v1".to_string(),
            kind: "Signer".to_string(),
            verbs: vec!["approve".to_string(), "sign".to_string()],
            preferred: true,
        });
    }

    resources
}

#[derive(Parser, Default, Debug)]
struct Args {
    output_dir: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    let client = Client::try_default().await?;
    let discovery = Discovery::new(client.clone()).run().await?;
    let namespaces = get_namespaces(&client, &discovery).await?;

    let metadata = Metadata {
        kube_version: get_version(&client).await?,
        context_name: get_context_name().await?,
        api_versions: api_versions(&discovery).await,
        preferred_versions: preferred_api_versions(&discovery).await,
        api_resources: api_resources(&discovery).await,
    };

    std::fs::create_dir_all(&args.output_dir)?;
    let metadata_file = std::fs::File::create(format!("{}/_metadata.json", args.output_dir))?;
    serde_json::to_writer_pretty(metadata_file, &metadata)?;

    for group in discovery.groups() {
        for (ar, caps) in group.recommended_resources() {
            if !caps.supports_operation(verbs::LIST) {
                continue;
            }

            println!("Processing {}", ar.plural);

            let apis: Vec<Api<DynamicObject>> = if caps.scope == Scope::Cluster {
                vec![Api::all_with(client.clone(), &ar)]
            } else {
                namespaces
                    .iter()
                    .map(|ns| Api::namespaced_with(client.clone(), ns, &ar))
                    .collect()
            };

            let mut items = Vec::new();
            for api in apis {
                items.extend(api.list(&Default::default()).await?.items);
            }

            if items.is_empty() {
                continue;
            }

            let new_items = items
                .iter_mut()
                .map(|x| serde_json::to_value(x).unwrap())
                .map(|x| match x {
                    serde_json::Value::Object(mut map) => {
                        map.insert(
                            "apiVersion".to_string(),
                            serde_json::Value::String(ar.api_version.clone()),
                        );
                        map.insert(
                            "kind".to_string(),
                            serde_json::Value::String(ar.kind.clone()),
                        );
                        if ar.plural == "secrets" {
                            map.remove("data");
                        }
                        map
                    }
                    _ => unreachable!(),
                })
                .collect::<Vec<_>>();

            let filename = if ar.group == "" {
                format!("{}/{}.json", args.output_dir, ar.plural)
            } else {
                format!("{}/{}.{}.json", args.output_dir, ar.plural, ar.group)
            };

            let file = std::fs::File::create(filename)?;
            serde_json::to_writer_pretty(file, &new_items)?;
        }
    }

    Ok(())
}
