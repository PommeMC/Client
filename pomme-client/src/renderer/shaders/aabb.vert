#version 450
#include "camera_ubo.glsl"
layout(push_constant) uniform BoundsLayout { uint word_stride; };
layout(set=1,binding=0) readonly buffer Bounds { uint bounds_words[]; };
layout(set=1,binding=1) readonly buffer Candidates { uint candidates[]; };
layout(location=0) flat out uint box_id;
void main(){uint id=candidates[gl_InstanceIndex];uint base=id*word_stride;vec3 aabb_min=uintBitsToFloat(uvec3(bounds_words[base],bounds_words[base+1],bounds_words[base+2]));vec3 aabb_max=uintBitsToFloat(uvec3(bounds_words[base+4],bounds_words[base+5],bounds_words[base+6]));ivec3 origin=ivec3(bounds_words[base+8],bounds_words[base+9],bounds_words[base+10]);vec3 side=vec3((gl_VertexIndex&1)!=0?1.0:0.0,(gl_VertexIndex&2)!=0?1.0:0.0,(gl_VertexIndex&4)!=0?1.0:0.0);vec3 corner=mix(aabb_min-vec3(0.1),aabb_max+vec3(0.1),side);vec3 rel=vec3(origin-camera_block.xyz)+corner-camera_pos.xyz;gl_Position=view_proj*vec4(rel,1.0);box_id=id;}
