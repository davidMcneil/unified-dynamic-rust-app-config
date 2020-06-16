mod configopt_parser;
mod serde_parser;
mod structopt_parser;

use configopt_parser::ConfigOptAttr;
use heck::{CamelCase, KebabCase, MixedCase, ShoutySnakeCase, SnakeCase};
use proc_macro2::{Span, TokenStream};
use proc_macro_roids::IdentExt;
use serde_parser::SerdeAttr;
use std::{convert::Infallible, str::FromStr};
use structopt_parser::StructOptAttr;
use syn::{parse_quote, spanned::Spanned, Expr, Field, Fields, Ident, Type, Variant};

pub use structopt_parser::{rename_all as structopt_rename_all, trim_structopt_attrs, StructOptTy};

pub fn configopt_ident(ident: &Ident) -> Ident {
    ident.prepend("ConfigOpt")
}

#[derive(Clone, Copy, PartialEq)]
pub enum CasingStyle {
    Camel,
    Kebab,
    Pascal,
    ScreamingSnake,
    Snake,
    Verbatim,
}

impl FromStr for CasingStyle {
    type Err = Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "camel" | "camelcase" => Self::Camel,
            "kebab" | "kebabcase" => Self::Kebab,
            "pascal" | "pascalcase" => Self::Pascal,
            "screamingsnake" | "screamingsnakecase" => Self::ScreamingSnake,
            "snake" | "snakecase" => Self::Snake,
            "verbatim" | "verbatimcase" => Self::Verbatim,
            _ => panic!("Invalid value for `rename_all` attribute"),
        })
    }
}

impl CasingStyle {
    pub fn rename(self, s: impl AsRef<str>) -> String {
        let s = s.as_ref();
        match self {
            CasingStyle::Kebab => s.to_kebab_case(),
            CasingStyle::Snake => s.to_snake_case(),
            CasingStyle::ScreamingSnake => s.to_shouty_snake_case(),
            CasingStyle::Camel => s.to_mixed_case(),
            CasingStyle::Pascal => s.to_camel_case(),
            CasingStyle::Verbatim => String::from(s),
        }
    }
}

pub fn inner_ty(ty: &mut Type) -> &mut Ident {
    match ty {
        Type::Path(type_path) => {
            if let Some(segment) = type_path.path.segments.last_mut() {
                &mut segment.ident
            } else {
                panic!(
                    "`#[configopt]` could not find a last segment in the type path to make partial"
                );
            }
        }
        _ => {
            panic!("`#[configopt]` only supports types specified by a path");
        }
    }
}

pub fn has_configopt_fields(parsed: &[ParsedField]) -> bool {
    parsed.iter().any(|f| f.ident() == "generate_config")
}

pub struct ParsedField {
    ident: Ident,
    structopt_ty: StructOptTy,
    configopt_inner_ty: Ident,
    span: Span,
    structopt_flatten: bool,
    serde_flatten: bool,
    subcommand: bool,
    structopt_rename: CasingStyle,
    structopt_name: String,
    serde_name: String,
    to_os_string: Option<Expr>,
}

impl ParsedField {
    pub fn new(field: &Field, structopt_rename: CasingStyle, serde_rename: CasingStyle) -> Self {
        let ident = field.ident.clone().expect("field ident to exist");
        let ty = &field.ty;
        let mut_ty = &mut field.ty.clone();
        let inner_ty = inner_ty(&mut mut_ty.clone()).clone();
        let structopt_attrs = structopt_parser::parse_attrs(&field.attrs);
        let serde_attrs = serde_parser::parse_attrs(&field.attrs);
        let configopt_attrs = configopt_parser::parse_attrs(&field.attrs);

        let structopt_name = structopt_attrs
            .iter()
            .find_map(|a| match &a {
                StructOptAttr::NameLitStr(name) => Some(name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| structopt_rename.rename(&ident.to_string()));

        let serde_name = serde_rename.rename(&ident.to_string());

        Self {
            ident,
            structopt_ty: StructOptTy::from_syn_ty(&ty),
            configopt_inner_ty: configopt_ident(&inner_ty),
            span: field.span(),
            structopt_rename,
            structopt_name,
            serde_name,
            structopt_flatten: structopt_attrs.iter().any(|a| match a {
                StructOptAttr::Flatten => true,
                _ => false,
            }),
            serde_flatten: serde_attrs.iter().any(|a| match a {
                SerdeAttr::Flatten => true,
                _ => false,
            }),
            subcommand: structopt_attrs.iter().any(|a| match a {
                StructOptAttr::Subcommand => true,
                _ => false,
            }),
            to_os_string: configopt_attrs.into_iter().find_map(|a| match a {
                ConfigOptAttr::ToOsString(expr) => Some(expr),
                _ => None,
            }),
        }
    }

    pub fn ident(&self) -> &Ident {
        &self.ident
    }

    pub fn structopt_ty(&self) -> &StructOptTy {
        &self.structopt_ty
    }

    pub fn configopt_inner_ty(&self) -> &Ident {
        &self.configopt_inner_ty
    }

    pub fn structopt_flatten(&self) -> bool {
        self.structopt_flatten
    }

    pub fn serde_flatten(&self) -> bool {
        self.serde_flatten
    }

    pub fn subcommand(&self) -> bool {
        self.subcommand
    }

    pub fn structopt_rename(&self) -> CasingStyle {
        self.structopt_rename
    }

    pub fn structopt_name(&self) -> &str {
        &self.structopt_name
    }

    pub fn serde_name(&self) -> &str {
        &self.serde_name
    }

    pub fn to_os_string(&self) -> Option<&Expr> {
        self.to_os_string.as_ref()
    }
}

impl Spanned for ParsedField {
    fn span(&self) -> Span {
        self.span
    }
}

#[derive(Clone, Copy)]
pub enum FieldType {
    Named,
    Unnamed,
    Unit,
}

impl From<&Fields> for FieldType {
    fn from(fields: &Fields) -> Self {
        match fields {
            Fields::Named(_) => Self::Named,
            Fields::Unnamed(_) => Self::Unnamed,
            Fields::Unit => Self::Unit,
        }
    }
}

pub struct ParsedVariant {
    full_ident: TokenStream,
    full_configopt_ident: TokenStream,
    span: Span,
    field_type: FieldType,
    structopt_name: String,
}

impl ParsedVariant {
    pub fn new(type_ident: &Ident, variant: &Variant) -> Self {
        let variant_ident = &variant.ident;
        let full_ident = parse_quote! {#type_ident::#variant_ident};
        let configopt_type_ident = configopt_ident(&type_ident);
        let full_configopt_ident = parse_quote! {#configopt_type_ident::#variant_ident};

        Self {
            full_ident,
            full_configopt_ident,
            span: variant.span(),
            field_type: (&variant.fields).into(),
            // TODO: Actually lookup the `structopt` name
            structopt_name: variant_ident.to_string().to_kebab_case(),
        }
    }

    pub fn full_ident(&self) -> &TokenStream {
        &self.full_ident
    }

    pub fn full_configopt_ident(&self) -> &TokenStream {
        &self.full_configopt_ident
    }

    pub fn field_type(&self) -> FieldType {
        self.field_type
    }

    pub fn structopt_name(&self) -> &str {
        &self.structopt_name
    }
}

impl Spanned for ParsedVariant {
    fn span(&self) -> Span {
        self.span
    }
}
